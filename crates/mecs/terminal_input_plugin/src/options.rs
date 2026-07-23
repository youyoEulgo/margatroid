#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TerminalMode {
    Raw,
    Cooked,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalInputOptions {
    pub(crate) mode: TerminalMode,
    pub(crate) alternate_screen: bool,
    pub(crate) mouse_capture: bool,
    pub(crate) bracketed_paste: bool,
    pub(crate) capacity: usize,
}

impl TerminalInputOptions {
    pub const DEFAULT_CAPACITY: usize = 1024;

    pub fn raw() -> Self {
        Self {
            mode: TerminalMode::Raw,
            alternate_screen: false,
            mouse_capture: false,
            bracketed_paste: false,
            capacity: Self::DEFAULT_CAPACITY,
        }
    }

    pub fn cooked() -> Self {
        Self {
            mode: TerminalMode::Cooked,
            alternate_screen: false,
            mouse_capture: false,
            bracketed_paste: false,
            capacity: Self::DEFAULT_CAPACITY,
        }
    }

    pub fn with_alternate_screen(mut self, enabled: bool) -> Self {
        self.alternate_screen = enabled;
        self
    }

    pub fn with_mouse_capture(mut self, enabled: bool) -> Self {
        self.mouse_capture = enabled;
        self
    }

    pub fn with_bracketed_paste(mut self, enabled: bool) -> Self {
        self.bracketed_paste = enabled;
        self
    }

    pub fn with_capacity(mut self, capacity: usize) -> Self {
        assert!(capacity > 0, "terminal event capacity must be positive");
        self.capacity = capacity;
        self
    }

    pub fn is_raw(&self) -> bool {
        self.mode == TerminalMode::Raw
    }

    pub fn uses_alternate_screen(&self) -> bool {
        self.alternate_screen
    }

    pub fn captures_mouse(&self) -> bool {
        self.mouse_capture
    }

    pub fn uses_bracketed_paste(&self) -> bool {
        self.bracketed_paste
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub(crate) fn validate(&self) -> Result<(), &'static str> {
        if self.mode == TerminalMode::Cooked
            && (self.alternate_screen || self.mouse_capture || self.bracketed_paste)
        {
            return Err(
                "alternate screen, mouse capture and bracketed paste require raw terminal mode",
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cooked_mode_rejects_raw_terminal_features() {
        assert!(TerminalInputOptions::cooked().validate().is_ok());
        assert!(TerminalInputOptions::cooked()
            .with_mouse_capture(true)
            .validate()
            .is_err());
    }
}
