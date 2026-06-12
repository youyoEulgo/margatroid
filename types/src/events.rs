/// Workspace 统一事件流（通过 SSE /workspace/{name}/stream 推送给前端）
///
/// WorkspaceEvent = EventPayload + EventContent
/// 所有事件走同一根通道，前端按 `type` 字段分派。

use serde::Serialize;

use crate::StreamChunk;

/// 事件元数据
#[derive(Debug, Clone, Serialize)]
pub struct EventPayload {
    pub event: String,
    pub member_id: String,
    pub delegation_id: String,
    pub timestamp: u64,
}

impl EventPayload {
    pub fn new(event: &str, member_id: &str, delegation_id: &str) -> Self {
        Self {
            event: event.to_string(),
            member_id: member_id.to_string(),
            delegation_id: delegation_id.to_string(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        }
    }
}

/// workspace 统一事件
#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceEvent {
    pub payload: EventPayload,

    #[serde(flatten)]
    pub content: EventContent,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum EventContent {
    /// LLM 流式 chunk——直接复用 types 里的 StreamChunk
    #[serde(rename = "stream_chunk")]
    StreamChunk {
        #[serde(flatten)]
        chunk: StreamChunk,
    },

    /// offer() 入发布区 或 result(done=true) 出发布区
    #[serde(rename = "board_update")]
    BoardUpdate { publish_count: usize },

    /// 任务链变化（delegate 右移 或 finish 左移）
    #[serde(rename = "chain_update")]
    ChainUpdate {
        from: String,
        to: String,
        brief: String,
        head_pos: usize,
    },

    /// 成员执行状态变化（开始处理 / 恢复空闲）
    #[serde(rename = "member_status")]
    MemberStatus { state: String },

    /// 人类成员收到委托（HumanProvider 创建请求时触发）
    #[serde(rename = "human_request")]
    HumanRequest {
        session_id: String,
        from: String,
        to: String,
        brief: String,
        detail: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_event_json_shape() {
        let payload = EventPayload {
            event: "stream_chunk".into(),
            member_id: "coder".into(),
            delegation_id: "d-001".into(),
            timestamp: 1700000000,
        };

        // stream_chunk
        let ev = WorkspaceEvent {
            payload: payload.clone(),
            content: EventContent::StreamChunk {
                chunk: StreamChunk {
                    id: "chat-1".into(),
                    model: "deepseek".into(),
                    choices: vec![],
                    usage: None,
                },
            },
        };
        let json = serde_json::to_string(&ev).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["type"], "stream_chunk");
        assert_eq!(v["payload"]["event"], "stream_chunk");
        assert_eq!(v["payload"]["member_id"], "coder");
        assert_eq!(v["id"], "chat-1");

        // board_update
        let payload = EventPayload {
            event: "board_update".into(),
            member_id: String::new(),
            delegation_id: String::new(),
            timestamp: 1700000001,
        };
        let json = serde_json::to_string(&WorkspaceEvent {
            payload: payload.clone(),
            content: EventContent::BoardUpdate { publish_count: 3 },
        })
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["type"], "board_update");
        assert_eq!(v["payload"]["event"], "board_update");
        assert_eq!(v["publish_count"], 3);

        // chain_update
        let payload = EventPayload {
            event: "chain_update".into(),
            member_id: String::new(),
            delegation_id: String::new(),
            timestamp: 1700000002,
        };
        let json = serde_json::to_string(&WorkspaceEvent {
            payload: payload.clone(),
            content: EventContent::ChainUpdate {
                from: "manager".into(),
                to: "coder".into(),
                brief: "实现JWT".into(),
                head_pos: 2,
            },
        })
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["type"], "chain_update");
        assert_eq!(v["payload"]["event"], "chain_update");
        assert_eq!(v["from"], "manager");

        // member_status
        let payload = EventPayload {
            event: "member_status".into(),
            member_id: "coder".into(),
            delegation_id: "d-001".into(),
            timestamp: 1700000003,
        };
        let json = serde_json::to_string(&WorkspaceEvent {
            payload: payload.clone(),
            content: EventContent::MemberStatus {
                state: "working".into(),
            },
        })
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["type"], "member_status");
        assert_eq!(v["payload"]["event"], "member_status");
        assert_eq!(v["state"], "working");

        // human_request
        let payload = EventPayload {
            event: "human_request".into(),
            member_id: String::new(),
            delegation_id: String::new(),
            timestamp: 1700000004,
        };
        let json = serde_json::to_string(&WorkspaceEvent {
            payload,
            content: EventContent::HumanRequest {
                session_id: "h-001".into(),
                from: "manager".into(),
                to: "user".into(),
                brief: "看看设计".into(),
                detail: "这是UI图".into(),
            },
        })
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["type"], "human_request");
        assert_eq!(v["payload"]["event"], "human_request");
        assert_eq!(v["from"], "manager");
        assert_eq!(v["to"], "user");
    }
}
