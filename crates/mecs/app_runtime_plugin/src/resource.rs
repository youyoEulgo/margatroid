use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use core_plugin::{App, EventSnapshot, Resource};

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
            Err(TrySendError::Disconnected(())) => panic!("runtime wake channel disconnected"),
        }
    }

    pub fn open_gate(&self) {
        let previous = self
            .blocker_count
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                count.checked_sub(1)
            })
            .unwrap_or_else(|_| panic!("runtime gate open and close calls must be paired"));
        if previous == 1 {
            self.wake();
        }
    }

    pub fn close_gate(&self) {
        self.blocker_count.fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn is_gate_open(&self) -> bool {
        self.blocker_count.load(Ordering::Acquire) == 0
    }
}

impl Resource for RuntimeHandle {}

pub(crate) struct RuntimeControl {
    mode: RuntimeMode,
    frame_rate: Option<u64>,
    handle: RuntimeHandle,
    wake_receiver: Mutex<Receiver<()>>,
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
            wake_receiver: Mutex::new(wake_receiver),
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

    fn wait(&self) {
        self.wake_receiver
            .lock()
            .expect("runtime wake receiver lock poisoned")
            .recv()
            .expect("runtime wake channel disconnected");
    }

    pub(crate) fn run(&mut self, app: &mut App) {
        match self.mode {
            RuntimeMode::FixedFrame => self.run_fixed_frame(app),
            RuntimeMode::EventDriven => self.run_event_driven(app),
        }
    }

    fn run_fixed_frame(&mut self, app: &mut App) {
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
                RuntimeState::Waiting | RuntimeState::Sleeping => self.wait(),
                RuntimeState::Closed => return,
            }
        }
    }

    fn run_event_driven(&mut self, app: &mut App) {
        loop {
            self.sync_event_snapshot(app);
            match self.status() {
                RuntimeState::Working => app.fast_forward_tick(),
                RuntimeState::Waiting | RuntimeState::Sleeping => self.wait(),
                RuntimeState::Closed => return,
            }
        }
    }
}

impl Resource for RuntimeControl {}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::sync_channel;

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
    #[should_panic(expected = "must be paired")]
    fn opening_an_open_gate_panics() {
        let (_control, handle) = control(RuntimeMode::EventDriven);
        handle.open_gate();
    }
}
