use std::fmt;
use std::io::{self, IsTerminal, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crossterm::event::{
    self, DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
    EnableFocusChange, EnableMouseCapture, Event as CrosstermEvent, KeyCode, KeyEvent,
    KeyEventKind, KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
    self, disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use external_event_plugin::{ExternalEventSendError, ExternalEventSender};

use crate::events::{TerminalEvent, TerminalInputFailed, TerminalInputFailureKind, TerminalSize};
use crate::options::{TerminalInputOptions, TerminalMode};

const INPUT_POLL_INTERVAL: Duration = Duration::from_millis(25);
static TERMINAL_OWNED: AtomicBool = AtomicBool::new(false);

struct TerminalInner {
    stop: AtomicBool,
    dropped: AtomicU64,
    thread: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Clone)]
pub struct TerminalSessionHandle {
    inner: Arc<TerminalInner>,
}

impl TerminalSessionHandle {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(TerminalInner {
                stop: AtomicBool::new(false),
                dropped: AtomicU64::new(0),
                thread: Mutex::new(None),
            }),
        }
    }

    pub fn is_running(&self) -> bool {
        self.inner
            .thread
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .is_some_and(|thread| !thread.is_finished())
    }

    pub fn size(&self) -> Result<TerminalSize, TerminalError> {
        terminal::size()
            .map(|(columns, rows)| TerminalSize::new(columns, rows))
            .map_err(|error| TerminalError(error.to_string()))
    }

    pub fn dropped_count(&self) -> u64 {
        self.inner.dropped.load(Ordering::Acquire)
    }

    pub fn shutdown(&self) {
        self.inner.shutdown();
    }

    pub(crate) fn start(
        &self,
        options: TerminalInputOptions,
        event_sender: ExternalEventSender<TerminalEvent>,
        failure_sender: ExternalEventSender<TerminalInputFailed>,
    ) -> Result<(), TerminalInputFailed> {
        if let Err(message) = options.validate() {
            return Err(failure(TerminalInputFailureKind::InvalidOptions, message));
        }
        if !io::stdin().is_terminal() {
            return Err(failure(
                TerminalInputFailureKind::NotATerminal,
                "stdin is not a terminal",
            ));
        }
        let mut thread_slot = self
            .inner
            .thread
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if thread_slot.is_some() {
            return Ok(());
        }
        let ownership = TerminalOwnership::acquire().ok_or_else(|| {
            failure(
                TerminalInputFailureKind::AlreadyInUse,
                "stdin is already owned by another TerminalInputPlugin",
            )
        })?;
        self.inner.stop.store(false, Ordering::Release);
        let mode_guard = TerminalModeGuard::enter(&options).map_err(|error| {
            failure(
                TerminalInputFailureKind::Setup,
                format!("cannot configure terminal: {error}"),
            )
        })?;
        let inner = Arc::downgrade(&self.inner);
        let input_mode = options.mode;
        let panic_inner = inner.clone();
        let panic_sender = failure_sender.clone();
        let thread = std::thread::Builder::new()
            .name("mecs-terminal-input".into())
            .spawn(move || {
                let _ownership = ownership;
                let _mode_guard = mode_guard;
                if let Err(payload) = catch_unwind(AssertUnwindSafe(|| {
                    input_loop(inner, input_mode, event_sender, failure_sender)
                })) {
                    send_failure(
                        &panic_inner,
                        &panic_sender,
                        TerminalInputFailureKind::ThreadPanicked,
                        panic_message(payload),
                    );
                }
            })
            .map_err(|error| {
                failure(
                    TerminalInputFailureKind::Setup,
                    format!("cannot start terminal input thread: {error}"),
                )
            })?;
        *thread_slot = Some(thread);
        Ok(())
    }
}

struct TerminalOwnership;

impl TerminalOwnership {
    fn acquire() -> Option<Self> {
        TERMINAL_OWNED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self)
    }
}

impl Drop for TerminalOwnership {
    fn drop(&mut self) {
        TERMINAL_OWNED.store(false, Ordering::Release);
    }
}

impl TerminalInner {
    fn shutdown(&self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self
            .thread
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            if thread.join().is_err() {
                tracing::error!("terminal input thread panicked");
            }
        }
    }
}

impl Drop for TerminalInner {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self
            .thread
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ = thread.join();
        }
    }
}

fn input_loop(
    inner: std::sync::Weak<TerminalInner>,
    mode: TerminalMode,
    event_sender: ExternalEventSender<TerminalEvent>,
    failure_sender: ExternalEventSender<TerminalInputFailed>,
) {
    match mode {
        TerminalMode::Raw => raw_input_loop(inner, event_sender, failure_sender),
        TerminalMode::Cooked => cooked_input_loop(inner, event_sender, failure_sender),
    }
}

fn raw_input_loop(
    inner: std::sync::Weak<TerminalInner>,
    event_sender: ExternalEventSender<TerminalEvent>,
    failure_sender: ExternalEventSender<TerminalInputFailed>,
) {
    let mut cooked_line = String::new();
    loop {
        let Some(state) = inner.upgrade() else {
            return;
        };
        if state.stop.load(Ordering::Acquire) {
            return;
        }
        drop(state);

        match event::poll(INPUT_POLL_INTERVAL) {
            Ok(false) => continue,
            Ok(true) => {}
            Err(error) => {
                send_failure(
                    &inner,
                    &failure_sender,
                    TerminalInputFailureKind::Poll,
                    error,
                );
                return;
            }
        }
        let input = match event::read() {
            Ok(input) => input,
            Err(error) => {
                send_failure(
                    &inner,
                    &failure_sender,
                    TerminalInputFailureKind::Read,
                    error,
                );
                return;
            }
        };
        let events = translate_event(input, TerminalMode::Raw, &mut cooked_line);
        for terminal_event in events {
            if !send_terminal_event(&inner, &event_sender, terminal_event) {
                return;
            }
        }
    }
}

#[cfg(unix)]
fn cooked_input_loop(
    inner: std::sync::Weak<TerminalInner>,
    event_sender: ExternalEventSender<TerminalEvent>,
    failure_sender: ExternalEventSender<TerminalInputFailed>,
) {
    use std::os::fd::{AsFd, AsRawFd};

    use nix::errno::Errno;
    use nix::sys::select::{select, FdSet};
    use nix::sys::time::TimeVal;

    let stdin = io::stdin();
    let mut pending = Vec::<u8>::new();
    let mut last_size = terminal::size().ok();
    loop {
        let Some(state) = inner.upgrade() else {
            return;
        };
        if state.stop.load(Ordering::Acquire) {
            return;
        }
        drop(state);

        let stdin_fd = stdin.as_fd();
        let mut read_fds = FdSet::new();
        read_fds.insert(stdin_fd);
        let mut timeout = TimeVal::new(0, 25_000);
        match select(None, &mut read_fds, None, None, &mut timeout) {
            Ok(0) | Err(Errno::EINTR) => {
                emit_resize_if_changed(&inner, &event_sender, &mut last_size);
                continue;
            }
            Ok(_) => {}
            Err(error) => {
                send_failure(
                    &inner,
                    &failure_sender,
                    TerminalInputFailureKind::Poll,
                    error,
                );
                return;
            }
        }

        let mut buffer = [0_u8; 4096];
        let count = match nix::unistd::read(stdin_fd.as_raw_fd(), &mut buffer) {
            Ok(count) => count,
            Err(Errno::EINTR) => continue,
            Err(error) => {
                send_failure(
                    &inner,
                    &failure_sender,
                    TerminalInputFailureKind::Read,
                    error,
                );
                return;
            }
        };
        if count == 0 {
            if !pending.is_empty()
                && !send_terminal_event(
                    &inner,
                    &event_sender,
                    TerminalEvent::Line(String::from_utf8_lossy(&pending).into_owned()),
                )
            {
                return;
            }
            let _ = send_terminal_event(&inner, &event_sender, TerminalEvent::EndOfInput);
            return;
        }
        pending.extend_from_slice(&buffer[..count]);
        while let Some(newline) = pending.iter().position(|byte| *byte == b'\n') {
            let mut line: Vec<_> = pending.drain(..=newline).collect();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            if !send_terminal_event(
                &inner,
                &event_sender,
                TerminalEvent::Line(String::from_utf8_lossy(&line).into_owned()),
            ) {
                return;
            }
        }
        emit_resize_if_changed(&inner, &event_sender, &mut last_size);
    }
}

#[cfg(not(unix))]
fn cooked_input_loop(
    inner: std::sync::Weak<TerminalInner>,
    event_sender: ExternalEventSender<TerminalEvent>,
    failure_sender: ExternalEventSender<TerminalInputFailed>,
) {
    let mut cooked_line = String::new();
    loop {
        let Some(state) = inner.upgrade() else {
            return;
        };
        if state.stop.load(Ordering::Acquire) {
            return;
        }
        drop(state);
        match event::poll(INPUT_POLL_INTERVAL) {
            Ok(false) => continue,
            Ok(true) => {}
            Err(error) => {
                send_failure(
                    &inner,
                    &failure_sender,
                    TerminalInputFailureKind::Poll,
                    error,
                );
                return;
            }
        }
        match event::read() {
            Ok(input) => {
                for terminal_event in translate_event(input, TerminalMode::Cooked, &mut cooked_line)
                {
                    if !send_terminal_event(&inner, &event_sender, terminal_event) {
                        return;
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
                let _ = send_terminal_event(&inner, &event_sender, TerminalEvent::EndOfInput);
                return;
            }
            Err(error) => {
                send_failure(
                    &inner,
                    &failure_sender,
                    TerminalInputFailureKind::Read,
                    error,
                );
                return;
            }
        }
    }
}

fn send_terminal_event(
    inner: &std::sync::Weak<TerminalInner>,
    sender: &ExternalEventSender<TerminalEvent>,
    event: TerminalEvent,
) -> bool {
    match sender.try_send(event) {
        Ok(()) => true,
        Err(ExternalEventSendError::Full(_)) => {
            increment_dropped(inner);
            true
        }
        Err(ExternalEventSendError::Closed(_)) => false,
    }
}

fn emit_resize_if_changed(
    inner: &std::sync::Weak<TerminalInner>,
    sender: &ExternalEventSender<TerminalEvent>,
    previous: &mut Option<(u16, u16)>,
) {
    let Ok(current) = terminal::size() else {
        return;
    };
    if Some(current) != *previous {
        *previous = Some(current);
        let _ = send_terminal_event(
            inner,
            sender,
            TerminalEvent::Resize(TerminalSize::new(current.0, current.1)),
        );
    }
}

fn translate_event(
    input: CrosstermEvent,
    mode: TerminalMode,
    cooked_line: &mut String,
) -> Vec<TerminalEvent> {
    match input {
        CrosstermEvent::Key(key) if mode == TerminalMode::Raw => vec![TerminalEvent::Key(key)],
        CrosstermEvent::Key(key) => translate_cooked_key(key, cooked_line),
        CrosstermEvent::Mouse(mouse) if mode == TerminalMode::Raw => {
            vec![TerminalEvent::Mouse(mouse)]
        }
        CrosstermEvent::Paste(value) if mode == TerminalMode::Raw => {
            vec![TerminalEvent::Paste(value)]
        }
        CrosstermEvent::Paste(value) => {
            cooked_line.push_str(&value);
            Vec::new()
        }
        CrosstermEvent::Resize(columns, rows) => {
            vec![TerminalEvent::Resize(TerminalSize::new(columns, rows))]
        }
        CrosstermEvent::FocusGained => vec![TerminalEvent::FocusGained],
        CrosstermEvent::FocusLost => vec![TerminalEvent::FocusLost],
        _ => Vec::new(),
    }
}

fn translate_cooked_key(key: KeyEvent, line: &mut String) -> Vec<TerminalEvent> {
    if key.kind != KeyEventKind::Press && key.kind != KeyEventKind::Repeat {
        return Vec::new();
    }
    match key.code {
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            vec![TerminalEvent::EndOfInput]
        }
        KeyCode::Char(value)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            line.push(value);
            Vec::new()
        }
        KeyCode::Backspace => {
            line.pop();
            Vec::new()
        }
        KeyCode::Enter => vec![TerminalEvent::Line(std::mem::take(line))],
        _ => Vec::new(),
    }
}

fn send_failure(
    inner: &std::sync::Weak<TerminalInner>,
    sender: &ExternalEventSender<TerminalInputFailed>,
    kind: TerminalInputFailureKind,
    error: impl fmt::Display,
) {
    match sender.try_send(failure(kind, error.to_string())) {
        Ok(()) => {}
        Err(ExternalEventSendError::Full(_)) => increment_dropped(inner),
        Err(ExternalEventSendError::Closed(_)) => {}
    }
}

fn increment_dropped(inner: &std::sync::Weak<TerminalInner>) {
    if let Some(inner) = inner.upgrade() {
        inner.dropped.fetch_add(1, Ordering::Relaxed);
    }
}

fn failure(kind: TerminalInputFailureKind, message: impl Into<String>) -> TerminalInputFailed {
    TerminalInputFailed {
        kind,
        message: message.into(),
    }
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "terminal input thread panicked with a non-string payload".into()
    }
}

struct TerminalModeGuard {
    raw: bool,
    alternate_screen: bool,
    mouse_capture: bool,
    bracketed_paste: bool,
    focus_change: bool,
}

impl TerminalModeGuard {
    fn enter(options: &TerminalInputOptions) -> io::Result<Self> {
        let mut guard = Self {
            raw: false,
            alternate_screen: false,
            mouse_capture: false,
            bracketed_paste: false,
            focus_change: false,
        };
        if options.mode == TerminalMode::Raw {
            enable_raw_mode()?;
            guard.raw = true;
            execute!(io::stdout(), EnableFocusChange)?;
            guard.focus_change = true;
        }
        if options.alternate_screen {
            execute!(io::stdout(), EnterAlternateScreen)?;
            guard.alternate_screen = true;
        }
        if options.mouse_capture {
            execute!(io::stdout(), EnableMouseCapture)?;
            guard.mouse_capture = true;
        }
        if options.bracketed_paste {
            execute!(io::stdout(), EnableBracketedPaste)?;
            guard.bracketed_paste = true;
        }
        Ok(guard)
    }

    fn restore(&mut self) {
        let mut stdout = io::stdout();
        if self.bracketed_paste {
            let _ = execute!(stdout, DisableBracketedPaste);
            self.bracketed_paste = false;
        }
        if self.mouse_capture {
            let _ = execute!(stdout, DisableMouseCapture);
            self.mouse_capture = false;
        }
        if self.alternate_screen {
            let _ = execute!(stdout, LeaveAlternateScreen);
            self.alternate_screen = false;
        }
        if self.focus_change {
            let _ = execute!(stdout, DisableFocusChange);
            self.focus_change = false;
        }
        if self.raw {
            let _ = disable_raw_mode();
            self.raw = false;
        }
        let _ = stdout.flush();
    }
}

impl Drop for TerminalModeGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalError(String);

impl fmt::Display for TerminalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for TerminalError {}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyEventState, MouseButton, MouseEvent, MouseEventKind};

    use super::*;

    #[test]
    fn terminal_ownership_is_process_wide_and_released_on_drop() {
        let first = TerminalOwnership::acquire().unwrap();
        assert!(TerminalOwnership::acquire().is_none());
        drop(first);
        assert!(TerminalOwnership::acquire().is_some());
    }

    #[test]
    fn raw_mode_preserves_key_and_resize_events() {
        let key = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL);
        assert_eq!(
            translate_event(
                CrosstermEvent::Key(key),
                TerminalMode::Raw,
                &mut String::new()
            ),
            [TerminalEvent::Key(key)]
        );
        assert_eq!(
            translate_event(
                CrosstermEvent::Resize(120, 40),
                TerminalMode::Raw,
                &mut String::new()
            ),
            [TerminalEvent::Resize(TerminalSize::new(120, 40))]
        );
    }

    #[test]
    fn cooked_mode_builds_lines_and_reports_end_of_input() {
        let mut line = String::new();
        for value in ['h', 'i'] {
            assert!(translate_event(
                CrosstermEvent::Key(KeyEvent::new(KeyCode::Char(value), KeyModifiers::NONE)),
                TerminalMode::Cooked,
                &mut line
            )
            .is_empty());
        }
        assert_eq!(
            translate_event(
                CrosstermEvent::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
                TerminalMode::Cooked,
                &mut line
            ),
            [TerminalEvent::Line("hi".into())]
        );
        assert_eq!(
            translate_event(
                CrosstermEvent::Key(KeyEvent {
                    code: KeyCode::Char('d'),
                    modifiers: KeyModifiers::CONTROL,
                    kind: KeyEventKind::Press,
                    state: KeyEventState::NONE,
                }),
                TerminalMode::Cooked,
                &mut line
            ),
            [TerminalEvent::EndOfInput]
        );
    }

    #[test]
    fn cooked_mode_ignores_mouse_events() {
        let mouse = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 1,
            row: 2,
            modifiers: KeyModifiers::NONE,
        };
        assert!(translate_event(
            CrosstermEvent::Mouse(mouse),
            TerminalMode::Cooked,
            &mut String::new()
        )
        .is_empty());
    }
}
