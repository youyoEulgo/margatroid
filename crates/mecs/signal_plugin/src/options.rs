use crate::ProcessSignal;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignalOptions {
    pub(crate) signals: Vec<ProcessSignal>,
    pub(crate) capacity: usize,
}

impl SignalOptions {
    pub const DEFAULT_CAPACITY: usize = 64;

    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_signals(mut self, signals: impl IntoIterator<Item = ProcessSignal>) -> Self {
        self.signals.clear();
        for signal in signals {
            if !self.signals.contains(&signal) {
                self.signals.push(signal);
            }
        }
        assert!(!self.signals.is_empty(), "signal list cannot be empty");
        self
    }

    pub fn with_capacity(mut self, capacity: usize) -> Self {
        assert!(capacity > 0, "signal event capacity must be positive");
        self.capacity = capacity;
        self
    }

    pub fn signals(&self) -> &[ProcessSignal] {
        &self.signals
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

impl Default for SignalOptions {
    fn default() -> Self {
        Self {
            signals: vec![ProcessSignal::Interrupt, ProcessSignal::Terminate],
            capacity: Self::DEFAULT_CAPACITY,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_signals_are_deduplicated() {
        let options = SignalOptions::new().with_signals([
            ProcessSignal::User1,
            ProcessSignal::User1,
            ProcessSignal::User2,
        ]);
        assert_eq!(
            options.signals(),
            [ProcessSignal::User1, ProcessSignal::User2]
        );
    }
}
