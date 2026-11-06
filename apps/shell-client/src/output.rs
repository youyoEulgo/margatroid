pub(crate) struct TerminalOutput {
    parser: vt100::Parser,
    history: String,
    previous: (String, bool),
    limit: usize,
}

impl TerminalOutput {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            // One byte can complete a CSI scroll of up to 24 rows. An extra
            // retained row lets scrollback offset 1 act as a scroll observer.
            parser: vt100::Parser::new(24, 80, 25),
            history: String::new(),
            previous: (String::new(), false),
            limit,
        }
    }

    pub(crate) fn process(&mut self, bytes: &[u8]) -> Result<(String, bool), std::io::Error> {
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
                    let row = screen.rows(0, 80).next().unwrap_or_default();
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
        text.push_str(&screen.contents());
        if text.len() > self.limit {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "terminal transcript exceeds output/cache budget",
            ));
        }
        let replace =
            alternate || alternate != self.previous.1 || !text.starts_with(&self.previous.0);
        let value = if replace {
            text.clone()
        } else {
            text[self.previous.0.len()..].to_string()
        };
        self.previous = (text, alternate);
        Ok((value, replace))
    }
}

#[cfg(test)]
mod tests {
    use super::TerminalOutput;

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
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("budget"));
        assert!(terminal.history.len() <= 128);
    }
}
