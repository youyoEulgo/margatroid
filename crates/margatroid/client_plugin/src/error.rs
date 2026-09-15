use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientError {
    InvalidClientType,
    InvalidResourceId,
    DuplicateName,
    ConnectionMissing,
    DuplicateClient,
}

impl fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidClientType => {
                formatter.write_str("client type is not a stable identifier")
            }
            Self::InvalidResourceId => formatter.write_str("client resource id could not be built"),
            Self::DuplicateName => formatter.write_str("client name is already in use"),
            Self::ConnectionMissing => {
                formatter.write_str("connection disappeared before registration")
            }
            Self::DuplicateClient => {
                formatter.write_str("client entity already exists for this resource id")
            }
        }
    }
}

impl std::error::Error for ClientError {}
