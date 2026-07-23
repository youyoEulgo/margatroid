use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use external_event_plugin::{ExternalEventSendError, ExternalEventSender};
use signal_hook::iterator::{Handle, Signals};

use crate::{ProcessSignal, ProcessSignalReceived};

struct SignalInner {
    iterator: Mutex<Option<Handle>>,
    thread: Mutex<Option<JoinHandle<()>>>,
    dropped: AtomicU64,
}

#[derive(Clone)]
pub struct SignalHandle {
    inner: Arc<SignalInner>,
}

impl SignalHandle {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(SignalInner {
                iterator: Mutex::new(None),
                thread: Mutex::new(None),
                dropped: AtomicU64::new(0),
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

    pub fn dropped_count(&self) -> u64 {
        self.inner.dropped.load(Ordering::Acquire)
    }

    pub fn shutdown(&self) {
        self.inner.shutdown();
    }

    pub(crate) fn start(
        &self,
        configured: &[ProcessSignal],
        sender: ExternalEventSender<ProcessSignalReceived>,
    ) -> io::Result<()> {
        let mut thread_slot = self
            .inner
            .thread
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if thread_slot.is_some() {
            return Ok(());
        }

        let signal_map = resolve_signals(configured)?;
        let mut signals = Signals::new(signal_map.keys().copied())?;
        let iterator = signals.handle();
        let inner = Arc::downgrade(&self.inner);
        let thread = std::thread::Builder::new()
            .name("mecs-signal-listener".into())
            .spawn(move || {
                for raw_signal in signals.forever() {
                    let Some(signal) = signal_map.get(&raw_signal).copied() else {
                        continue;
                    };
                    match sender.try_send(ProcessSignalReceived { signal }) {
                        Ok(()) => {}
                        Err(ExternalEventSendError::Full(_)) => {
                            if let Some(inner) = inner.upgrade() {
                                inner.dropped.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                        Err(ExternalEventSendError::Closed(_)) => break,
                    }
                }
            })?;
        *self
            .inner
            .iterator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(iterator);
        *thread_slot = Some(thread);
        Ok(())
    }
}

impl SignalInner {
    fn shutdown(&self) {
        if let Some(handle) = self
            .iterator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            handle.close();
        }
        if let Some(thread) = self
            .thread
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            if thread.join().is_err() {
                tracing::error!("signal listener thread panicked");
            }
        }
    }
}

impl Drop for SignalInner {
    fn drop(&mut self) {
        if let Some(handle) = self
            .iterator
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            handle.close();
        }
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

fn resolve_signals(configured: &[ProcessSignal]) -> io::Result<HashMap<i32, ProcessSignal>> {
    let mut resolved = HashMap::new();
    for signal in configured {
        let raw = raw_signal(*signal)?;
        resolved.entry(raw).or_insert(*signal);
    }
    Ok(resolved)
}

#[cfg(unix)]
fn raw_signal(signal: ProcessSignal) -> io::Result<i32> {
    use signal_hook::consts::{
        SIGHUP, SIGINT, SIGKILL, SIGQUIT, SIGSTOP, SIGTERM, SIGUSR1, SIGUSR2, SIGWINCH,
    };

    let raw = match signal {
        ProcessSignal::Interrupt => SIGINT,
        ProcessSignal::Terminate => SIGTERM,
        ProcessSignal::Hangup => SIGHUP,
        ProcessSignal::Quit => SIGQUIT,
        ProcessSignal::WindowChanged => SIGWINCH,
        ProcessSignal::User1 => SIGUSR1,
        ProcessSignal::User2 => SIGUSR2,
        ProcessSignal::Raw(raw) => raw,
    };
    if raw <= 0 || matches!(raw, SIGKILL | SIGSTOP) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("signal {raw} cannot be captured"),
        ));
    }
    Ok(raw)
}

#[cfg(not(unix))]
fn raw_signal(signal: ProcessSignal) -> io::Result<i32> {
    use signal_hook::consts::{SIGINT, SIGTERM};

    match signal {
        ProcessSignal::Interrupt => Ok(SIGINT),
        ProcessSignal::Terminate => Ok(SIGTERM),
        _ => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("signal {signal:?} is not supported on this platform"),
        )),
    }
}
