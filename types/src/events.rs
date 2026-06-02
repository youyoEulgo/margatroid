/// Workspace 统一事件流（通过 SSE /workspace/{name}/stream 推送给前端）
///
/// 每个事件实现 Serialize + Deserialize，构造后 `serde_json::to_string` 直接序列化。
/// 高频 chat chunk 不在此列——它走独立的 per-task channel。

use serde::{Deserialize, Serialize};

/// board 发布区任务数变化（offer 入队 / result 出队时触发）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoardUpdateEvent {
    #[serde(rename = "type")]
    pub event_type: &'static str,
    pub publish_count: usize,
}

impl BoardUpdateEvent {
    pub fn new(publish_count: usize) -> Self {
        Self {
            event_type: "board_update",
            publish_count,
        }
    }
}

/// 人类成员收到委托（HumanProvider 创建请求时触发）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HumanRequestEvent {
    #[serde(rename = "type")]
    pub event_type: &'static str,
    pub session_id: String,
}

impl HumanRequestEvent {
    pub fn new(session_id: String) -> Self {
        Self {
            event_type: "human_request",
            session_id,
        }
    }
}

/// 任务链变化（delegate 右移 或 finish(done=true) 左移时触发）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainUpdateEvent {
    #[serde(rename = "type")]
    pub event_type: &'static str,
}

impl ChainUpdateEvent {
    pub fn new() -> Self {
        Self {
            event_type: "chain_update",
        }
    }
}

/// 成员执行状态变化（开始处理 / 恢复空闲）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemberStatusEvent {
    #[serde(rename = "type")]
    pub event_type: &'static str,
    pub member_id: String,
    pub state: String, // "working" | "idle"
}

impl MemberStatusEvent {
    pub fn new(member_id: String, state: String) -> Self {
        Self {
            event_type: "member_status",
            member_id,
            state,
        }
    }
}
