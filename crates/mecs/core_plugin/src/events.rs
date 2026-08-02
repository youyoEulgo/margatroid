use std::any::{Any, TypeId};
use std::collections::{HashMap, VecDeque};
use std::marker::PhantomData;
use std::sync::{Arc, Mutex, RwLock};

use crate::CoreError;

pub trait Event: Send + Sync + 'static {}

impl<T, E> Event for Result<T, E>
where
    T: Send + Sync + 'static,
    E: Send + Sync + 'static,
{
}

enum EventState {
    Pending,
    Normal {
        body: Box<dyn ErasedEvent>,
        remaining_delay_frames: u64,
    },
}

type EventNode = Arc<Mutex<EventState>>;

trait ErasedEvent: Send + Sync + 'static {
    fn push_into(self: Box<Self>, registry: &mut EventReadStorageRegistry);
}

impl<E: Event> ErasedEvent for E {
    fn push_into(self: Box<Self>, registry: &mut EventReadStorageRegistry) {
        registry.push(*self);
    }
}

trait ErasedEventReadStorage: Any + Send + Sync + 'static {
    fn clear(&mut self);
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

struct EventReadStorage<E: Event> {
    events: Vec<E>,
}

impl<E: Event> EventReadStorage<E> {
    fn new() -> Self {
        Self { events: Vec::new() }
    }

    fn push(&mut self, event: E) {
        self.events.push(event);
    }
}

impl<E: Event> ErasedEventReadStorage for EventReadStorage<E> {
    fn clear(&mut self) {
        self.events.clear();
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

pub(crate) struct EventReadStorageRegistry {
    storages: HashMap<TypeId, Box<dyn ErasedEventReadStorage>>,
}

impl EventReadStorageRegistry {
    pub(crate) fn new() -> Self {
        Self {
            storages: HashMap::new(),
        }
    }

    pub(crate) fn reader<E: Event>(&self) -> EventReader<'_, E> {
        let events = self
            .storages
            .get(&TypeId::of::<E>())
            .map(|storage| {
                &storage
                    .as_any()
                    .downcast_ref::<EventReadStorage<E>>()
                    .expect("event storage type must match its TypeId")
                    .events[..]
            })
            .unwrap_or(&[]);
        EventReader { events }
    }

    fn push<E: Event>(&mut self, event: E) {
        let storage = self
            .storages
            .entry(TypeId::of::<E>())
            .or_insert_with(|| Box::new(EventReadStorage::<E>::new()));
        storage
            .as_any_mut()
            .downcast_mut::<EventReadStorage<E>>()
            .expect("event storage type must match its TypeId")
            .push(event);
    }

    pub(crate) fn clear(&mut self) {
        for storage in self.storages.values_mut() {
            storage.clear();
        }
    }
}

#[derive(Clone)]
pub struct EventSnapshot {
    pub normal_event_count: usize,
    pub pending_event_count: usize,
    pub nearest_normal_event_delay: Option<u64>,
}

#[must_use = "pending event handles must be completed with `complete` or `complete_after`"]
pub struct EventHandle<E: Event> {
    node: EventNode,
    snapshot: Arc<Mutex<EventSnapshot>>,
    marker: PhantomData<E>,
}

impl<T, E> EventHandle<Result<T, E>>
where
    T: Send + Sync + 'static,
    E: Send + Sync + 'static,
{
    pub fn complete(self, result: Result<T, E>) {
        self.complete_after(result, 0);
    }

    pub fn complete_after(self, result: Result<T, E>, extra_delay_frames: u64) {
        let mut snapshot = self.snapshot.lock().expect("event snapshot lock poisoned");
        let mut state = self.node.lock().expect("event node lock poisoned");
        if !matches!(*state, EventState::Pending) {
            CoreError::PendingEventAlreadyCompleted.panic();
        }
        *state = EventState::Normal {
            body: Box::new(result),
            remaining_delay_frames: extra_delay_frames,
        };
        snapshot.pending_event_count -= 1;
        snapshot.normal_event_count += 1;
        if snapshot
            .nearest_normal_event_delay
            .is_none_or(|nearest| extra_delay_frames < nearest)
        {
            snapshot.nearest_normal_event_delay = Some(extra_delay_frames);
        }
    }
}

pub struct EventQueue {
    pending: VecDeque<EventNode>,
    snapshot: Arc<Mutex<EventSnapshot>>,
}

#[derive(Clone)]
pub struct EventEmitter {
    queue: Arc<RwLock<EventQueue>>,
}

impl EventEmitter {
    pub(crate) fn new(queue: Arc<RwLock<EventQueue>>) -> Self {
        Self { queue }
    }

    pub fn emit_event<E: Event>(&self, event: E) {
        self.queue
            .write()
            .expect("event queue lock poisoned")
            .send_event(event);
    }

    pub fn emit_event_after<E: Event>(&self, event: E, delay: u64) {
        self.queue
            .write()
            .expect("event queue lock poisoned")
            .send_event_after(event, delay);
    }
}

impl EventQueue {
    pub(crate) fn new() -> Self {
        Self {
            pending: VecDeque::new(),
            snapshot: Arc::new(Mutex::new(EventSnapshot {
                normal_event_count: 0,
                pending_event_count: 0,
                nearest_normal_event_delay: None,
            })),
        }
    }

    pub fn snapshot(&self) -> EventSnapshot {
        self.snapshot
            .lock()
            .expect("event snapshot lock poisoned")
            .clone()
    }

    pub(crate) fn pull_events(&mut self, registry: &mut EventReadStorageRegistry) {
        let mut snapshot = self.snapshot.lock().expect("event snapshot lock poisoned");
        snapshot.nearest_normal_event_delay = None;
        let count = self.pending.len();
        for _ in 0..count {
            let node = self
                .pending
                .pop_front()
                .expect("event queue length must match its bounded iteration");
            let mut state = node.lock().expect("event node lock poisoned");
            match &mut *state {
                EventState::Pending => {
                    drop(state);
                    self.pending.push_back(node);
                }
                EventState::Normal {
                    remaining_delay_frames: 0,
                    ..
                } => {
                    let EventState::Normal { body, .. } =
                        std::mem::replace(&mut *state, EventState::Pending)
                    else {
                        unreachable!("matched event state must remain normal");
                    };
                    drop(state);
                    body.push_into(registry);
                    snapshot.normal_event_count -= 1;
                }
                EventState::Normal {
                    remaining_delay_frames,
                    ..
                } => {
                    *remaining_delay_frames -= 1;
                    if snapshot
                        .nearest_normal_event_delay
                        .is_none_or(|nearest| *remaining_delay_frames < nearest)
                    {
                        snapshot.nearest_normal_event_delay = Some(*remaining_delay_frames);
                    }
                    drop(state);
                    self.pending.push_back(node);
                }
            }
        }
    }

    pub fn send_event<E: Event>(&mut self, event: E) {
        let mut snapshot = self.snapshot.lock().expect("event snapshot lock poisoned");
        self.pending
            .push_back(Arc::new(Mutex::new(EventState::Normal {
                body: Box::new(event),
                remaining_delay_frames: 0,
            })));
        snapshot.normal_event_count += 1;
        snapshot.nearest_normal_event_delay = Some(0);
    }

    pub fn send_event_after<E: Event>(&mut self, event: E, extra_delay_frames: u64) {
        let mut snapshot = self.snapshot.lock().expect("event snapshot lock poisoned");
        self.pending
            .push_back(Arc::new(Mutex::new(EventState::Normal {
                body: Box::new(event),
                remaining_delay_frames: extra_delay_frames,
            })));
        snapshot.normal_event_count += 1;
        if snapshot
            .nearest_normal_event_delay
            .is_none_or(|nearest| extra_delay_frames < nearest)
        {
            snapshot.nearest_normal_event_delay = Some(extra_delay_frames);
        }
    }

    pub fn send_pending<T, E>(&mut self) -> EventHandle<Result<T, E>>
    where
        T: Send + Sync + 'static,
        E: Send + Sync + 'static,
    {
        let mut snapshot = self.snapshot.lock().expect("event snapshot lock poisoned");
        let node = Arc::new(Mutex::new(EventState::Pending));
        self.pending.push_back(Arc::clone(&node));
        snapshot.pending_event_count += 1;
        EventHandle {
            node,
            snapshot: Arc::clone(&self.snapshot),
            marker: PhantomData,
        }
    }

    pub(crate) fn fast_forward(&mut self) {
        let mut snapshot = self.snapshot.lock().expect("event snapshot lock poisoned");
        let Some(nearest_delay) = snapshot.nearest_normal_event_delay else {
            return;
        };
        if nearest_delay == 0 {
            return;
        }
        for node in &self.pending {
            let mut state = node.lock().expect("event node lock poisoned");
            if let EventState::Normal {
                remaining_delay_frames,
                ..
            } = &mut *state
            {
                *remaining_delay_frames -= nearest_delay;
            }
        }
        snapshot.nearest_normal_event_delay = Some(0);
    }
}

pub struct EventReader<'a, E: Event> {
    events: &'a [E],
}

impl<E: Event> EventReader<'_, E> {
    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

impl<'a, E: Event> IntoIterator for EventReader<'a, E> {
    type Item = &'a E;
    type IntoIter = std::slice::Iter<'a, E>;

    fn into_iter(self) -> Self::IntoIter {
        self.events.iter()
    }
}

impl<'reader, 'storage, E: Event> IntoIterator for &'reader EventReader<'storage, E> {
    type Item = &'reader E;
    type IntoIter = std::slice::Iter<'reader, E>;

    fn into_iter(self) -> Self::IntoIter {
        self.events.iter()
    }
}
