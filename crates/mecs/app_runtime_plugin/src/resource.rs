use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use core_plugin::{App, Event, EventEmitter, EventSnapshot, Resource};

use crate::RuntimeError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeMode {
    FixedFrame,
    EventDriven,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeState {
    Working,
    Waiting,
    Sleeping,
    Closed,
}

#[derive(Clone)]
pub struct RuntimeHandle {
    blocker_count: Arc<AtomicUsize>,
    wake_sender: SyncSender<()>,
}

impl RuntimeHandle {
    pub(crate) fn new(wake_sender: SyncSender<()>) -> Self {
        Self {
            blocker_count: Arc::new(AtomicUsize::new(0)),
            wake_sender,
        }
    }

    pub fn wake(&self) {
        match self.wake_sender.try_send(()) {
            Ok(()) | Err(TrySendError::Full(())) => {}
            Err(TrySendError::Disconnected(())) => RuntimeError::WakeChannelDisconnected.panic(),
        }
    }

    pub fn open_gate(&self) {
        let previous = self
            .blocker_count
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                count.checked_sub(1)
            })
            .unwrap_or_else(|_| RuntimeError::GateOperationUnbalanced.panic());
        if previous == 1 {
            self.wake();
        }
    }

    pub fn close_gate(&self) {
        self.blocker_count
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                count.checked_add(1)
            })
            .expect("runtime blocker count overflow");
    }

    pub(crate) fn is_gate_open(&self) -> bool {
        self.blocker_count.load(Ordering::Acquire) == 0
    }
}

impl Resource for RuntimeHandle {}

#[derive(Clone)]
pub struct RuntimeEventSender {
    emitter: EventEmitter,
    runtime: RuntimeHandle,
}

impl RuntimeEventSender {
    pub(crate) fn new(emitter: EventEmitter, runtime: RuntimeHandle) -> Self {
        Self { emitter, runtime }
    }

    pub fn send_event<E: Event>(&self, event: E) {
        self.emitter.emit_event(event);
        self.runtime.wake();
    }

    pub fn send_event_after<E: Event>(&self, event: E, delay: u64) {
        self.emitter.emit_event_after(event, delay);
        self.runtime.wake();
    }
}

pub(crate) struct RuntimeControl {
    mode: RuntimeMode,
    frame_rate: Option<u64>,
    handle: RuntimeHandle,
    wake_receiver: Mutex<Option<Receiver<()>>>,
    event_snapshot: EventSnapshot,
}

impl RuntimeControl {
    pub(crate) fn new(
        mode: RuntimeMode,
        frame_rate: Option<u64>,
        handle: RuntimeHandle,
        wake_receiver: Receiver<()>,
        event_snapshot: EventSnapshot,
    ) -> Self {
        Self {
            mode,
            frame_rate,
            handle,
            wake_receiver: Mutex::new(Some(wake_receiver)),
            event_snapshot,
        }
    }

    fn sync_event_snapshot(&mut self, app: &App) {
        self.event_snapshot = app.world().event_snapshot();
    }

    fn status(&self) -> RuntimeState {
        if !self.handle.is_gate_open() {
            return RuntimeState::Waiting;
        }
        match self.mode {
            RuntimeMode::FixedFrame => RuntimeState::Working,
            RuntimeMode::EventDriven if self.event_snapshot.normal_event_count > 0 => {
                RuntimeState::Working
            }
            RuntimeMode::EventDriven if self.event_snapshot.pending_event_count > 0 => {
                RuntimeState::Waiting
            }
            RuntimeMode::EventDriven => RuntimeState::Sleeping,
        }
    }

    fn wait(wake_receiver: &Receiver<()>) {
        wake_receiver
            .recv()
            .unwrap_or_else(|_| RuntimeError::WakeChannelDisconnected.panic());
    }

    pub(crate) fn run(&mut self, app: &mut App) {
        let wake_receiver = self
            .wake_receiver
            .lock()
            .expect("runtime wake receiver lock poisoned")
            .take()
            .expect("runtime wake receiver must only be taken once");
        match self.mode {
            RuntimeMode::FixedFrame => self.run_fixed_frame(app, &wake_receiver),
            RuntimeMode::EventDriven => self.run_event_driven(app, &wake_receiver),
        }
    }

    fn run_fixed_frame(&mut self, app: &mut App, wake_receiver: &Receiver<()>) {
        let frame_rate = self
            .frame_rate
            .expect("fixed-frame runtime must have a frame rate");
        let frame_duration = Duration::from_secs_f64(1.0 / frame_rate as f64);
        loop {
            self.sync_event_snapshot(app);
            match self.status() {
                RuntimeState::Working => {
                    app.tick();
                    std::thread::sleep(frame_duration);
                }
                RuntimeState::Waiting | RuntimeState::Sleeping => Self::wait(wake_receiver),
                RuntimeState::Closed => return,
            }
        }
    }

    fn run_event_driven(&mut self, app: &mut App, wake_receiver: &Receiver<()>) {
        self.run_initial_frame(app);
        loop {
            self.sync_event_snapshot(app);
            match self.status() {
                RuntimeState::Working => app.fast_forward_tick(),
                RuntimeState::Waiting | RuntimeState::Sleeping => Self::wait(wake_receiver),
                RuntimeState::Closed => return,
            }
        }
    }

    fn run_initial_frame(&mut self, app: &mut App) {
        app.tick();
        self.sync_event_snapshot(app);
    }
}

impl Resource for RuntimeControl {}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::sync_channel;

    use core_plugin::World;

    use super::*;

    fn control(mode: RuntimeMode) -> (RuntimeControl, RuntimeHandle) {
        let (sender, receiver) = sync_channel(1);
        let handle = RuntimeHandle::new(sender);
        let control = RuntimeControl::new(
            mode,
            (mode == RuntimeMode::FixedFrame).then_some(60),
            handle.clone(),
            receiver,
            EventSnapshot {
                normal_event_count: 0,
                pending_event_count: 0,
                nearest_normal_event_delay: None,
            },
        );
        (control, handle)
    }

    #[test]
    fn event_driven_state_distinguishes_work_wait_and_sleep() {
        let (mut control, _handle) = control(RuntimeMode::EventDriven);
        assert_eq!(control.status(), RuntimeState::Sleeping);
        control.event_snapshot.pending_event_count = 1;
        assert_eq!(control.status(), RuntimeState::Waiting);
        control.event_snapshot.normal_event_count = 1;
        assert_eq!(control.status(), RuntimeState::Working);
    }

    #[test]
    fn gate_waits_for_every_blocker() {
        let (control, handle) = control(RuntimeMode::EventDriven);
        handle.close_gate();
        handle.close_gate();
        assert_eq!(control.status(), RuntimeState::Waiting);
        handle.open_gate();
        assert_eq!(control.status(), RuntimeState::Waiting);
        handle.open_gate();
        assert_eq!(control.status(), RuntimeState::Sleeping);
    }

    #[test]
    #[should_panic(expected = "unbalanced")]
    fn opening_an_open_gate_panics() {
        let (_control, handle) = control(RuntimeMode::EventDriven);
        handle.open_gate();
    }

    #[test]
    fn event_driven_initial_frame_runs_without_queued_events() {
        let calls = Arc::new(AtomicUsize::new(0));
        let startup_calls = Arc::clone(&calls);
        let mut app = App::new();
        app.add_once_schedule("startup".into())
            .add_system("startup", move |_world: &mut World| {
                startup_calls.fetch_add(1, Ordering::Relaxed);
            });
        let (mut control, _handle) = control(RuntimeMode::EventDriven);

        control.run_initial_frame(&mut app);

        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert_eq!(control.status(), RuntimeState::Sleeping);
    }
}
