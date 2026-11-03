use crate::types::TerminalOutput;
use tool_core::ToolErrorKind;

#[test]
fn ordinary_scrolling_is_append_only_across_chunks() {
    let mut terminal = TerminalOutput::new(8192);
    let mut result = String::new();
    let mut expected = String::new();
    for index in 1..=80 {
        let line = format!("[{index:03}] line\r\n");
        expected.push_str(&format!("[{index:03}] line\n"));
        let (text, replace) = terminal.process(line.as_bytes()).unwrap();
        assert!(!replace, "line {index} unexpectedly replaced output");
        result.push_str(&text);
    }
    assert_eq!(result, expected.trim_end_matches('\n'));
}

#[test]
fn a_single_read_preserves_eighty_lines_and_repeated_lines() {
    for repeated in [false, true] {
        let input = (1..=80)
            .map(|index| {
                if repeated {
                    "same\r\n".to_string()
                } else {
                    format!("[{index:03}]\r\n")
                }
            })
            .collect::<String>();
        let mut terminal = TerminalOutput::new(8192);
        let (text, replace) = terminal.process(input.as_bytes()).unwrap();
        assert!(!replace);
        assert_eq!(text, input.replace('\r', "").trim_end_matches('\n'));
        assert_eq!(text.lines().count(), 80);
        assert_eq!(terminal.process(b"").unwrap(), (String::new(), false));
    }
}

#[test]
fn ansi_and_fragmented_utf8_are_rendered_without_escape_codes() {
    let input = "\x1b[31m你好\x1b[0m\r\n".repeat(80);
    let mut terminal = TerminalOutput::new(8192);
    let mut result = String::new();
    for byte in input.as_bytes() {
        let (text, replace) = terminal.process(&[*byte]).unwrap();
        assert!(!replace);
        result.push_str(&text);
    }
    assert_eq!(result, "你好\n".repeat(80).trim_end_matches('\n'));
}

#[test]
fn wrapped_rows_and_multirow_scroll_keep_exact_content() {
    let mut terminal = TerminalOutput::new(8192);
    let long = "x".repeat(80 * 30 + 7);
    assert_eq!(terminal.process(long.as_bytes()).unwrap(), (long, false));
    let mut terminal = TerminalOutput::new(8192);
    let input = (1..=24)
        .map(|index| format!("line{index}\r\n"))
        .collect::<String>();
    let (first, _) = terminal.process(input.as_bytes()).unwrap();
    let (next, replace) = terminal.process(b"\x1b[24S").unwrap();
    assert!(!replace);
    assert_eq!(
        format!("{first}{next}")
            .lines()
            .filter(|row| !row.is_empty())
            .count(),
        24
    );
}

#[test]
fn real_overwrites_and_alternate_screen_still_replace() {
    let mut terminal = TerminalOutput::new(8192);
    assert_eq!(
        terminal.process(b"progress 1").unwrap(),
        ("progress 1".into(), false)
    );
    assert_eq!(
        terminal.process(b"\rprogress 2\x1b[K").unwrap(),
        ("progress 2".into(), true)
    );
    assert_eq!(
        terminal
            .process(b"\x1b[?1049h\x1b[2J\x1b[HSCREEN_ONE")
            .unwrap(),
        ("SCREEN_ONE".into(), true)
    );
    assert_eq!(
        terminal.process(b"\x1b[HSCREEN_TWO\x1b[K").unwrap(),
        ("SCREEN_TWO".into(), true)
    );
    assert_eq!(
        terminal.process(b"\x1b[?1049l").unwrap(),
        ("progress 2".into(), true)
    );
    assert_eq!(terminal.process(b" done").unwrap(), (" done".into(), false));
}

#[test]
fn history_budget_fails_explicitly_instead_of_dropping_rows() {
    let mut terminal = TerminalOutput::new(128);
    let error = terminal
        .process("row\r\n".repeat(80).as_bytes())
        .unwrap_err();
    assert_eq!(error.kind, ToolErrorKind::OutputTooLarge);
    assert!(error.message.contains("budget"));
    assert!(terminal.history.len() <= 128);
}
