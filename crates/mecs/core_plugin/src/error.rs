use std::fmt;

#[non_exhaustive]
#[derive(Debug)]
pub enum CoreError {
    AppAlreadyStarted,
    ScheduleAlreadyExists { name: String },
    ScheduleNotFound { name: String },
    EntityCapacityExhausted,
    PendingEventAlreadyCompleted,
}

impl CoreError {
    pub(crate) fn panic(self) -> ! {
        panic!("{self}")
    }
}

impl fmt::Display for CoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AppAlreadyStarted => formatter.write_str("app has already started"),
            Self::ScheduleAlreadyExists { name } => {
                write!(formatter, "schedule `{name}` already exists")
            }
            Self::ScheduleNotFound { name } => {
                write!(formatter, "schedule `{name}` does not exist")
            }
            Self::EntityCapacityExhausted => formatter.write_str("entity capacity exhausted"),
            Self::PendingEventAlreadyCompleted => {
                formatter.write_str("pending event has already been completed")
            }
        }
    }
}

impl std::error::Error for CoreError {}
