use serde::{Deserialize, Serialize};

use crate::RequestId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidArgument,
    InvalidBundle,
    UnsupportedVersion,
    Unauthorized,
    Forbidden,
    NotFound,
    AlreadyExists,
    ResourceInUse,
    Conflict,
    QueueFull,
    Unavailable,
    Internal,
}

impl ErrorCode {
    pub const fn http_status(self) -> u16 {
        match self {
            Self::InvalidArgument | Self::InvalidBundle | Self::UnsupportedVersion => 400,
            Self::Unauthorized => 401,
            Self::Forbidden => 403,
            Self::NotFound => 404,
            Self::AlreadyExists | Self::ResourceInUse | Self::Conflict => 409,
            Self::QueueFull => 429,
            Self::Unavailable => 503,
            Self::Internal => 500,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    pub code: ErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<RequestId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

impl ApiError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            request_id: None,
            details: None,
        }
    }

    pub fn with_request_id(mut self, request_id: RequestId) -> Self {
        self.request_id = Some(request_id);
        self
    }

    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: ApiError,
}
