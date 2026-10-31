use std::{fmt, io, path::PathBuf};

#[derive(Debug)]
pub enum AddressError {
    InvalidFormat,
    InvalidPart,
    EntityMissing,
    DuplicateAddress,
    PathOutsideRoot { root: PathBuf, path: PathBuf },
    FileMissing(PathBuf),
    NotFile(PathBuf),
    Io(io::Error),
}

impl fmt::Display for AddressError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFormat => write!(
                formatter,
                "invalid address format: expected type:scope/name[:tag]"
            ),
            Self::InvalidPart => write!(formatter, "invalid address part or unsupported file type"),
            Self::EntityMissing => write!(formatter, "no entity has this address"),
            Self::DuplicateAddress => write!(formatter, "multiple entities have this address"),
            Self::PathOutsideRoot { root, path } => write!(
                formatter,
                "path {} is outside root {}",
                path.display(),
                root.display()
            ),
            Self::FileMissing(path) => write!(formatter, "file missing: {}", path.display()),
            Self::NotFile(path) => write!(formatter, "not a regular file: {}", path.display()),
            Self::Io(error) => write!(formatter, "address filesystem error: {error}"),
        }
    }
}

impl std::error::Error for AddressError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for AddressError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
