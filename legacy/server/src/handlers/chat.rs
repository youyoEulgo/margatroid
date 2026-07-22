use anyhow::anyhow;
use axum::{Json, extract::State};
use serde::Deserialize;
use types::ChatRequest;

use crate::state::{AnyhowError, AppState};

#[derive(Debug, Deserialize)]
pub struct ChatApiRequest {
    pub provider: String,
    #[serde(flatten)]
    pub req: ChatRequest,
}
pub async fn chat(
    State(state): State<AppState>,
    Json(payload): Json<ChatApiRequest>,
) -> Result<Json<types::ChatResponse>, AnyhowError> {
    let provider = state
        .get_provider(&payload.provider)
        .await
        .ok_or_else(|| anyhow!("provider '{}' not found", payload.provider))?;
    let resp = provider.chat_boxed(payload.req).await?;
    Ok(Json(resp))
}
