use std::pin::Pin;

use axum::{
    Json,
    extract::{Path, State},
    response::sse::{Event, Sse},
};
use futures::Stream;
use runtime::memory::Worklog;
use serde::Deserialize;
use tokio_stream::{StreamExt, wrappers::BroadcastStream};

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

/// GET /ws/{name}/recent — 最近的工作日志（用于前端轮询）
pub async fn recent(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, AnyhowError> {
    let ws = state
        .workspace(&name)
        .await
        .ok_or_else(|| anyhow::anyhow!("workspace '{}' not running", name))?;
    let entries = ws.board.db().recent(20);
    Ok(Json(serde_json::json!(entries)))
}

/// GET /ws/{name}/conversation — 最近的对话消息（用于前端展示 LLM 回复）
pub async fn conversation(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, AnyhowError> {
    let ws = state
        .workspace(&name)
        .await
        .ok_or_else(|| anyhow::anyhow!("workspace '{}' not running", name))?;
    let msgs = ws.board.db().recent_conversations(50);
    Ok(Json(serde_json::json!(msgs)))
}

/// GET /ws/{name}/events/{task_id} — SSE 流，推送 LLM 对话消息和完成事件
pub async fn events(
    State(state): State<AppState>,
    Path((name, task_id)): Path<(String, String)>,
) -> Sse<Pin<Box<dyn Stream<Item = Result<Event, std::convert::Infallible>> + Send>>> {
    let ws = match state.workspace(&name).await {
        Some(w) => w,
        None => {
            return Sse::new(Box::pin(futures::stream::once(async {
                Ok(Event::default().data(r#"{"type":"error","content":"workspace not found"}"#))
            })));
        }
    };

    let rx = match ws.board.register_listener(&task_id).await {
        Some(rx) => rx,
        None => {
            return Sse::new(Box::pin(futures::stream::once(async {
                Ok(Event::default().data(r#"{"type":"error","content":"task not found"}"#))
            })));
        }
    };

    // 检查竞态：Manager 可能已经 finish
    let already_done = ws
        .board
        .db()
        .recent(20)
        .into_iter()
        .any(|e| e.delegation_id == task_id && !e.summary.is_empty());

    let broadcast = BroadcastStream::new(rx).map(|item| {
        let data = match item {
            Ok(s) => s,
            Err(_) => r#"{"type":"error","content":"stream lagged"}"#.into(),
        };
        Ok(Event::default().data(data))
    });

    let stream: Pin<Box<dyn Stream<Item = _> + Send>> = if already_done {
        let done = futures::stream::once(async {
            Ok(Event::default().data(r#"{"type":"done"}"#))
        });
        Box::pin(done.chain(broadcast))
    } else {
        Box::pin(broadcast)
    };

    Sse::new(stream)
}
