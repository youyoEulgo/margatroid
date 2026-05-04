//! 统一错误响应

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;

pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
}

impl ApiError {
    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: msg.into(),
        }
    }

    pub fn not_found(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: msg.into(),
        }
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: msg.into(),
        }
    }

    pub fn service_unavailable(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: msg.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Json(json!({
            "error": self.message,
            "status": self.status.as_u16(),
        }));
        (self.status, body).into_response()
    }
}

impl From<providers::ProviderError> for ApiError {
    fn from(e: providers::ProviderError) -> Self {
        match &e {
            providers::ProviderError::InvalidRequest(msg) => Self::bad_request(msg.clone()),
            providers::ProviderError::Unsupported(msg) => Self::bad_request(msg.clone()),
            providers::ProviderError::Api { code, message, .. } => Self {
                status: StatusCode::from_u16(*code as u16).unwrap_or(StatusCode::BAD_GATEWAY),
                message: message.clone(),
            },
            _ => Self::internal(e.to_string()),
        }
    }
}

pub type ApiResult<T> = Result<T, ApiError>;
