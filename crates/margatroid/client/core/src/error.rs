use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientError {
    InvalidRequest,
    InvalidClientType,
    InvalidName,
    DuplicateName,
    DuplicateClient,
    ConnectionMissing,
    AddressConflict,
}

impl fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidRequest => "client registration request ID is empty",
            Self::InvalidClientType => "client type is not a stable identifier",
            Self::InvalidName => "client name is not a valid address segment",
            Self::DuplicateName => "client name is already in use",
            Self::DuplicateClient => "connection is already registered",
            Self::ConnectionMissing => "connection disappeared before registration",
            Self::AddressConflict => "client address is already occupied or invalid",
        })
    }
}

impl std::error::Error for ClientError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientSendError {
    Full,
    Closed,
}
impl fmt::Display for ClientSendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Full => "client outbound queue is full",
            Self::Closed => "client outbound queue is closed",
        })
    }
}
impl std::error::Error for ClientSendError {}
