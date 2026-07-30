use std::any::{Any, TypeId};
use std::collections::{HashMap, VecDeque};

pub trait Event: Send + Sync + 'static {}

struct QueuedEvent {
    body: Box<dyn Any + Send + Sync>,
    remaining_delay_frames: u64,
}

trait ErasedEventReadStorage: Any + Send + Sync + 'static {
    fn clear(&mut self);
    fn push_boxed(&mut self, body: Box<dyn Any + Send + Sync>);
    fn as_any(&self) -> &dyn Any;
    #[allow(dead_code)]
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

    fn push_boxed(&mut self, body: Box<dyn Any + Send + Sync>) {
        let event = body
            .downcast::<E>()
            .expect("event body type must match its TypeId");
        self.push(*event);
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

    pub(crate) fn register<E: Event>(&mut self) {
        self.storages
            .entry(TypeId::of::<E>())
            .or_insert_with(|| Box::new(EventReadStorage::<E>::new()));
    }

    pub(crate) fn reader<E: Event>(&self) -> EventReader<'_, E> {
        let storage = self
            .storages
            .get(&TypeId::of::<E>())
            .unwrap_or_else(|| panic!("event `{}` is not registered", std::any::type_name::<E>()))
            .as_any()
            .downcast_ref::<EventReadStorage<E>>()
            .expect("event storage type must match its TypeId");
        EventReader { storage }
    }

    fn push_event(&mut self, event: QueuedEvent) {
        let event_type = event.body.as_ref().type_id();
        let storage = self
            .storages
            .get_mut(&event_type)
            .unwrap_or_else(|| panic!("received an event whose type is not registered"));
        storage.push_boxed(event.body);
    }

    pub(crate) fn clear(&mut self) {
        for storage in self.storages.values_mut() {
            storage.clear();
        }
    }
}

pub struct EventQueue {
    pending: VecDeque<QueuedEvent>,
}

impl EventQueue {
    pub(crate) fn new() -> Self {
        Self {
            pending: VecDeque::new(),
        }
    }

    pub(crate) fn pull_events(&mut self, registry: &mut EventReadStorageRegistry) {
        let count = self.pending.len();
        for _ in 0..count {
            if let Some(event) = self.pop_event() {
                registry.push_event(event);
            }
        }
    }

    fn pop_event(&mut self) -> Option<QueuedEvent> {
        let mut event = self.pending.pop_front()?;
        if event.remaining_delay_frames == 0 {
            return Some(event);
        }
        event.remaining_delay_frames -= 1;
        self.pending.push_back(event);
        None
    }

    fn push_event(&mut self, body: Box<dyn Any + Send + Sync>, delay_frames: u64) {
        self.pending.push_back(QueuedEvent {
            body,
            remaining_delay_frames: delay_frames,
        });
    }

    pub fn send_event<E: Event>(&mut self, event: E) {
        self.push_event(Box::new(event), 0);
    }

    pub fn send_event_after<E: Event>(&mut self, event: E, extra_delay_frames: u64) {
        self.push_event(Box::new(event), extra_delay_frames);
    }
}

pub struct EventReader<'a, E: Event> {
    storage: &'a EventReadStorage<E>,
}

impl<E: Event> EventReader<'_, E> {
    pub fn len(&self) -> usize {
        self.storage.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.storage.events.is_empty()
    }
}

impl<'a, E: Event> IntoIterator for EventReader<'a, E> {
    type Item = &'a E;
    type IntoIter = std::slice::Iter<'a, E>;

    fn into_iter(self) -> Self::IntoIter {
        self.storage.events.iter()
    }
}

impl<'reader, 'storage, E: Event> IntoIterator for &'reader EventReader<'storage, E> {
    type Item = &'reader E;
    type IntoIter = std::slice::Iter<'reader, E>;

    fn into_iter(self) -> Self::IntoIter {
        self.storage.events.iter()
    }
}
