use axum::{
    Json,
    extract::{Path, State},
};
use serde::Deserialize;

use crate::state::{AnyhowError, AppState};

// ── 请求体 ──

#[derive(Debug, Deserialize)]
pub struct ChatBody {
    pub brief: String,
    #[serde(default)]
    pub detail: String,
}

// ── handlers ──

/// POST /ws/{name}/chat — 以 user 身份向 manager 发消息
pub async fn chat(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(payload): Json<ChatBody>,
) -> Result<Json<serde_json::Value>, AnyhowError> {
    let ws = state
        .workspace(&name)
        .await
        .ok_or_else(|| anyhow::anyhow!("workspace '{}' not running", name))?;
    let task_id = ws
        .send_user_message("user", "manager", &payload.brief, &payload.detail)
        .await?;
    Ok(Json(serde_json::json!({ "ok": true, "task_id": task_id })))
}

/// GET /ws/{name}/status
pub async fn status(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, AnyhowError> {
    let ws = state
        .workspace(&name)
        .await
        .ok_or_else(|| anyhow::anyhow!("workspace '{}' not running", name))?;
    let s = ws.board.status().await;
    Ok(Json(serde_json::json!({ "publish_count": s.publish_count })))
}

/// GET /ws/{name}/tasks
pub async fn tasks(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, AnyhowError> {
    let ws = state
        .workspace(&name)
        .await
        .ok_or_else(|| anyhow::anyhow!("workspace '{}' not running", name))?;
    Ok(Json(serde_json::json!(ws.board.status().await)))
}
