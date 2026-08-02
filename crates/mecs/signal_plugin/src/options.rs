use crate::ProcessSignal;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignalOptions {
    pub(crate) signals: Vec<ProcessSignal>,
}

impl SignalOptions {
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

    pub fn signals(&self) -> &[ProcessSignal] {
        &self.signals
    }
}

impl Default for SignalOptions {
    fn default() -> Self {
        Self {
            signals: vec![ProcessSignal::Interrupt, ProcessSignal::Terminate],
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
