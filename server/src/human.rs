//! Human 交互端点
//!
//! HumanProvider 通过 POST 创建请求、GET 阻塞等待回复（Notify 唤醒）。
//! 前端通过 GET /api/human/requests 列出 + POST reply 提交回复。

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Notify;
use tracing::info;
use types::{ChatResponse, RequestMessage, RequestTool};

pub struct PendingTask {
    pub session_id: String,
    pub messages: Vec<RequestMessage>,
    pub tools: Vec<RequestTool>,
    status: String,
    response: Option<ChatResponse>,
    notify: Arc<Notify>,
    created_at: Instant,
}

pub type PendingMap = Arc<tokio::sync::RwLock<HashMap<String, PendingTask>>>;

pub fn new_pending_map() -> PendingMap {
    Arc::new(tokio::sync::RwLock::new(HashMap::new()))
}

// ── 请求/响应体 ──

#[derive(Deserialize)]
pub struct CreateRequest {
    pub messages: Vec<RequestMessage>,
    pub tools: Vec<RequestTool>,
}

#[derive(Serialize)]
pub struct PendingItem {
    pub session_id: String,
    pub message_count: usize,
    pub tool_count: usize,
    pub created_secs: u64,
}

#[derive(Deserialize)]
pub struct ReplyBody {
    pub response: ChatResponse,
}

// ── handlers ──

pub async fn create_request(
    State(pending): State<PendingMap>,
    Json(body): Json<CreateRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let session_id = uuid::Uuid::new_v4().to_string();
    let task = PendingTask {
        session_id: session_id.clone(),
        messages: body.messages,
        tools: body.tools,
        status: "pending".to_string(),
        response: None,
        notify: Arc::new(Notify::new()),
        created_at: Instant::now(),
    };
    pending.write().await.insert(session_id.clone(), task);

    info!("human request created: {}", &session_id[..8]);
    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "session_id": session_id,
        })),
    )
}

pub async fn wait_reply(
    State(pending): State<PendingMap>,
    Path(session_id): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    let notify = {
        let map = pending.read().await;
        match map.get(&session_id) {
            Some(t) => t.notify.clone(),
            None => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({
                        "error": "not found"
                    })),
                );
            }
        }
    };
    notify.notified().await;

    let map = pending.read().await;
    let task = match map.get(&session_id) {
        Some(t) => t,
        None => {
            return (
                StatusCode::GONE,
                Json(serde_json::json!({
                    "status": "timeout"
                })),
            );
        }
    };

    match task.status.as_str() {
        "completed" => (
            StatusCode::OK,
            Json(serde_json::json!({
                "status": "completed",
                "response": task.response.as_ref().unwrap(),
            })),
        ),
        _ => (
            StatusCode::OK,
            Json(serde_json::json!({ "status": task.status })),
        ),
    }
}

pub async fn list_requests(State(pending): State<PendingMap>) -> Json<Vec<PendingItem>> {
    let map = pending.read().await;
    let items: Vec<PendingItem> = map
        .iter()
        .filter(|(_, t)| t.status == "pending")
        .map(|(id, t)| PendingItem {
            session_id: id.clone(),
            message_count: t.messages.len(),
            tool_count: t.tools.len(),
            created_secs: t.created_at.elapsed().as_secs(),
        })
        .collect();
    Json(items)
}

pub async fn submit_reply(
    State(pending): State<PendingMap>,
    Path(session_id): Path<String>,
    Json(body): Json<ReplyBody>,
) -> (StatusCode, Json<serde_json::Value>) {
    let mut map = pending.write().await;
    let task = match map.get_mut(&session_id) {
        Some(t) => t,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": "not found"
                })),
            );
        }
    };

    if task.status != "pending" {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": format!("already {}", task.status)
            })),
        );
    }

    task.status = "completed".to_string();
    task.response = Some(body.response);
    task.notify.notify_one();

    info!("human request completed: {}", &session_id[..8]);
    (StatusCode::OK, Json(serde_json::json!({ "ok": true })))
}
