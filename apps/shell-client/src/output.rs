pub(crate) struct TerminalOutput {
    parser: vt100::Parser,
    history: String,
    previous: (String, bool),
    limit: usize,
    resized: bool,
}

impl TerminalOutput {
    pub(crate) fn new(limit: usize, rows: u16, cols: u16) -> Self {
        Self {
            parser: vt100::Parser::new(rows, cols, usize::from(rows) * 2),
            history: String::new(),
            previous: (String::new(), false),
            limit,
            resized: false,
        }
    }

    pub(crate) fn resize(&mut self, rows: u16, cols: u16) -> std::io::Result<()> {
        if rows == 0 || cols == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "terminal size must be nonzero",
            ));
        }
        if self.parser.screen().size() != (rows, cols) {
            self.parser.screen_mut().set_scrollback(0);
            self.parser.screen_mut().set_size(rows, cols);
            self.parser
                .screen_mut()
                .set_scrollback_len(usize::from(rows) * 2);
            self.resized = true;
        }
        Ok(())
    }

    pub(crate) fn process(&mut self, bytes: &[u8]) -> Result<(String, bool), std::io::Error> {
        let (_, cols) = self.parser.screen().size();
        for byte in bytes {
            let screen = self.parser.screen_mut();
            let alternate = screen.alternate_screen();
            screen.set_scrollback(1);
            let before = screen.scrollback();
            self.parser.process(std::slice::from_ref(byte));
            let screen = self.parser.screen_mut();
            if !alternate && !screen.alternate_screen() {
                // With no history the offset is zero and doesn't advance on
                // scroll. Query the clamped maximum to observe the first rows.
                if before == 0 {
                    screen.set_scrollback(usize::MAX);
                }
                let count = screen.scrollback().saturating_sub(before);
                for offset in (1..=count).rev() {
                    screen.set_scrollback(offset);
                    let row = screen.rows(0, cols).next().unwrap_or_default();
                    let separator = if screen.row_wrapped(0) { "" } else { "\n" };
                    if self
                        .history
                        .len()
                        .saturating_add(row.len())
                        .saturating_add(separator.len())
                        > self.limit
                    {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "terminal transcript exceeds output/cache budget",
                        ));
                    }
                    self.history.push_str(&row);
                    self.history.push_str(separator);
                }
            }
            screen.set_scrollback(0);
        }
        let screen = self.parser.screen();
        let alternate = screen.alternate_screen();
        let mut text = if alternate {
            String::new()
        } else {
            self.history.clone()
        };
        if alternate {
            let mut rows = screen.rows(0, cols).collect::<Vec<_>>();
            while rows.last().is_some_and(String::is_empty) {
                rows.pop();
            }
            text.push_str(&rows.join("\n"));
        } else {
            text.push_str(&screen.contents());
        }
        if text.len() > self.limit {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "terminal transcript exceeds output/cache budget",
            ));
        }
        let replace = self.resized
            || alternate
            || alternate != self.previous.1
            || !text.starts_with(&self.previous.0);
        let value = if replace {
            text.clone()
        } else {
            text[self.previous.0.len()..].to_string()
        };
        self.previous = (text, alternate);
        self.resized = false;
        Ok((value, replace))
    }
}

#[cfg(test)]
mod tests {
    use super::TerminalOutput;

    #[test]
    fn ordinary_scrolling_is_append_only_across_chunks() {
        let mut terminal = TerminalOutput::new(8192, 24, 80);
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
            let mut terminal = TerminalOutput::new(8192, 24, 80);
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
        let mut terminal = TerminalOutput::new(8192, 24, 80);
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
        let mut terminal = TerminalOutput::new(8192, 24, 80);
        let long = "x".repeat(80 * 30 + 7);
        assert_eq!(terminal.process(long.as_bytes()).unwrap(), (long, false));
        let mut terminal = TerminalOutput::new(8192, 24, 80);
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
        let mut terminal = TerminalOutput::new(8192, 24, 80);
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
        let mut terminal = TerminalOutput::new(128, 24, 80);
        let error = terminal
            .process("row\r\n".repeat(80).as_bytes())
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("budget"));
        assert!(terminal.history.len() <= 128);
    }
    #[test]
    fn resized_tui_keeps_physical_rows_and_forces_complete_replacement() {
        let mut terminal = TerminalOutput::new(32768, 24, 80);
        terminal.process(b"initial").unwrap();
        terminal.resize(40, 120).unwrap();
        let border = "─".repeat(118);
        let frame =
            format!("\x1b[?1049h\x1b[2J\x1b[H┌{border}┐\x1b[39;1H└{border}┘\x1b[40;1HNow Playing");
        let (text, replace) = terminal.process(frame.as_bytes()).unwrap();
        assert!(replace);
        let rows = text.split('\n').collect::<Vec<_>>();
        assert_eq!(rows.len(), 40);
        assert_eq!(rows[0], format!("┌{border}┐"));
        assert_eq!(rows[38], format!("└{border}┘"));
        assert_eq!(rows[39], "Now Playing");
        terminal.resize(10, 30).unwrap();
        let (text, replace) = terminal
            .process(b"\x1b[2J\x1b[Hsmall\x1b[10;1Hbottom")
            .unwrap();
        assert!(replace);
        let rows = text.split('\n').collect::<Vec<_>>();
        assert_eq!(rows.len(), 10);
        assert_eq!(rows[0], "small");
        assert_eq!(rows[9], "bottom");
    }

    #[test]
    fn alternate_screen_wraps_are_rows_while_plain_transcripts_remain_logical_lines() {
        let mut terminal = TerminalOutput::new(8192, 24, 80);
        let (text, replace) = terminal
            .process(format!("\x1b[?1049h{}END", "x".repeat(80)).as_bytes())
            .unwrap();
        assert!(replace);
        assert_eq!(text, format!("{}\nEND", "x".repeat(80)));
        terminal.process(b"\x1b[?1049l").unwrap();
        terminal.resize(40, 120).unwrap();
        let line = "a".repeat(120);
        let (text, replace) = terminal
            .process(format!("\x1b[2J\x1b[H{line}tail").as_bytes())
            .unwrap();
        assert!(replace);
        assert_eq!(text, format!("{line}tail"));
        let (text, replace) = terminal.process(b"more").unwrap();
        assert!(!replace);
        assert_eq!(text, "more");
    }

    #[test]
    fn tall_terminal_multirow_scrolling_preserves_all_lines() {
        let mut terminal = TerminalOutput::new(32768, 24, 80);
        terminal.resize(40, 120).unwrap();
        let input = (1..=40)
            .map(|index| format!("row{index:02}\r\n"))
            .collect::<String>();
        let (first, _) = terminal.process(input.as_bytes()).unwrap();
        let (next, replace) = terminal.process(b"\x1b[40S").unwrap();
        assert!(!replace);
        let text = format!("{first}{next}");
        for index in 1..=40 {
            assert_eq!(
                text.lines()
                    .filter(|row| *row == format!("row{index:02}"))
                    .count(),
                1
            );
        }
        assert!(terminal.resize(0, 120).is_err());
    }
    #[test]
    fn scrollback_is_twice_current_height_and_resize_preserves_pending_parser_state() {
        let mut terminal = TerminalOutput::new(1024 * 1024, 24, 80);
        terminal.process("row\r\n".repeat(1000).as_bytes()).unwrap();
        terminal.parser.screen_mut().set_scrollback(usize::MAX);
        assert_eq!(terminal.parser.screen().scrollback(), 48);
        terminal.parser.screen_mut().set_scrollback(0);
        terminal.process(b"\x1b[").unwrap();
        terminal.resize(40, 120).unwrap();
        let (text, replace) = terminal.process(b"2J\x1b[Hnormal\x1b[?1049hALT").unwrap();
        assert!(replace);
        assert_eq!(text, "ALT");
        terminal.resize(10, 30).unwrap();
        let (text, replace) = terminal.process(b"\x1b[?1049l").unwrap();
        assert!(replace);
        assert!(text.ends_with("normal"));
        terminal
            .process("next\r\n".repeat(1000).as_bytes())
            .unwrap();
        terminal.parser.screen_mut().set_scrollback(usize::MAX);
        assert_eq!(terminal.parser.screen().scrollback(), 20);
        terminal.parser.screen_mut().set_scrollback(0);
        terminal.resize(40, 120).unwrap();
        terminal
            .process("wide\r\n".repeat(1000).as_bytes())
            .unwrap();
        terminal.parser.screen_mut().set_scrollback(usize::MAX);
        assert_eq!(terminal.parser.screen().scrollback(), 80);
    }

    #[test]
    fn shrinking_scrollback_keeps_fragmented_utf8_and_text_history() {
        let mut terminal = TerminalOutput::new(1024 * 1024, 40, 120);
        let first = terminal
            .process("old\r\n".repeat(500).as_bytes())
            .unwrap()
            .0;
        let bytes = "你".as_bytes();
        terminal.process(&bytes[..1]).unwrap();
        terminal.resize(10, 30).unwrap();
        let (text, replace) = terminal.process(&bytes[1..]).unwrap();
        assert!(replace);
        assert!(text.contains('你'));
        assert!(text.starts_with(&first[..100]));
    }
}
