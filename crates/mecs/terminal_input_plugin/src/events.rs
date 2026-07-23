use crossterm::event::{KeyEvent, MouseEvent};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalSize {
    pub columns: u16,
    pub rows: u16,
}

impl TerminalSize {
    pub fn new(columns: u16, rows: u16) -> Self {
        Self { columns, rows }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalEvent {
    Key(KeyEvent),
    Paste(String),
    Mouse(MouseEvent),
    Resize(TerminalSize),
    FocusGained,
    FocusLost,
    Line(String),
    EndOfInput,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalInputFailureKind {
    NotATerminal,
    AlreadyInUse,
    InvalidOptions,
    Setup,
    Poll,
    Read,
    ThreadPanicked,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalInputFailed {
    pub kind: TerminalInputFailureKind,
    pub message: String,
}
