/// Workspace 统一事件流（通过 SSE /workspace/{name}/stream 推送给前端）
///
/// WorkspaceEvent = EventMetadata + EventContent
/// 顶层只有 metadata 和 content，前端按 metadata.event 分派，content 可直接反序列化。
use serde::Serialize;

use crate::StreamChunk;

/// 事件元数据
#[derive(Debug, Clone, Serialize)]
pub struct EventMetadata {
    pub event: String,
    pub member_id: String,
    pub delegation_id: String,
    pub timestamp: u64,
}

impl EventMetadata {
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

    #[cfg(test)]
    pub fn with_timestamp(mut self, ts: u64) -> Self {
        self.timestamp = ts;
        self
    }
}

/// workspace 统一事件
#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceEvent {
    pub metadata: EventMetadata,
    pub content: EventContent,
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum EventContent {
    /// LLM 流式 chunk——直接复用 types 里的 StreamChunk
    StreamChunk { chunk: StreamChunk },

    /// offer() 入发布区 或 result(done=true) 出发布区
    BoardUpdate { publish_count: usize },

    /// 任务链变化（delegate 右移 或 finish 左移）
    ChainUpdate {
        from: String,
        to: String,
        brief: String,
        head_pos: usize,
    },

    /// 成员执行状态变化（开始处理 / 恢复空闲）
    MemberStatus { state: String },

    /// 人类成员收到委托（HumanProvider 创建请求时触发）
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
        // stream_chunk
        let ev = WorkspaceEvent {
            metadata: EventMetadata::new("stream_chunk", "coder", "d-001")
                .with_timestamp(1700000000),
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
        assert_eq!(v["metadata"]["event"], "stream_chunk");
        assert_eq!(v["metadata"]["member_id"], "coder");
        assert_eq!(v["content"]["chunk"]["id"], "chat-1");
        assert!(v.get("type").is_none());

        // board_update
        let json = serde_json::to_string(&WorkspaceEvent {
            metadata: EventMetadata::new("board_update", "", "").with_timestamp(1700000001),
            content: EventContent::BoardUpdate { publish_count: 3 },
        })
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["metadata"]["event"], "board_update");
        assert_eq!(v["content"]["publish_count"], 3);
        assert!(v.get("type").is_none());

        // chain_update
        let json = serde_json::to_string(&WorkspaceEvent {
            metadata: EventMetadata::new("chain_update", "", "").with_timestamp(1700000002),
            content: EventContent::ChainUpdate {
                from: "manager".into(),
                to: "coder".into(),
                brief: "实现JWT".into(),
                head_pos: 2,
            },
        })
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["metadata"]["event"], "chain_update");
        assert_eq!(v["content"]["from"], "manager");
        assert!(v.get("type").is_none());

        // member_status
        let json = serde_json::to_string(&WorkspaceEvent {
            metadata: EventMetadata::new("member_status", "coder", "d-001")
                .with_timestamp(1700000003),
            content: EventContent::MemberStatus {
                state: "working".into(),
            },
        })
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["metadata"]["event"], "member_status");
        assert_eq!(v["content"]["state"], "working");
        assert!(v.get("type").is_none());

        // human_request
        let json = serde_json::to_string(&WorkspaceEvent {
            metadata: EventMetadata::new("human_request", "", "").with_timestamp(1700000004),
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
        assert_eq!(v["metadata"]["event"], "human_request");
        assert_eq!(v["content"]["from"], "manager");
        assert_eq!(v["content"]["to"], "user");
        assert!(v.get("type").is_none());
    }
}
