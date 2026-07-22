#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalEventOptions {
    pub(crate) capacity: usize,
    pub(crate) max_events_per_frame: usize,
}

impl ExternalEventOptions {
    pub const DEFAULT_CAPACITY: usize = 1024;
    pub const DEFAULT_MAX_EVENTS_PER_FRAME: usize = 256;

    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(mut self, capacity: usize) -> Self {
        assert!(capacity > 0, "external event capacity must be positive");
        self.capacity = capacity;
        self
    }

    pub fn with_max_events_per_frame(mut self, limit: usize) -> Self {
        assert!(
            limit > 0,
            "external event max events per frame must be positive"
        );
        self.max_events_per_frame = limit;
        self
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn max_events_per_frame(&self) -> usize {
        self.max_events_per_frame
    }
}

impl Default for ExternalEventOptions {
    fn default() -> Self {
        Self {
            capacity: Self::DEFAULT_CAPACITY,
            max_events_per_frame: Self::DEFAULT_MAX_EVENTS_PER_FRAME,
        }
    }
}
