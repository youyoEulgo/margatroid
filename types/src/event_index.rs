//! Workspace 事件信道与事件类型名录
//!
//! 单通道设计：CHANNEL_WORKSPACE_STREAM — WorkspaceEvent = EventPayload + EventContent
//! 前端通过 GET /workspace/{name}/stream 订阅，按 `type` 字段分派。

// ── 信道 ──

/// workspace 统一事件流通道 key
/// 前端通过 GET /workspace/{name}/stream 订阅
pub const CHANNEL_WORKSPACE_STREAM: &str = "workspace_stream";

// ── 事件类型 ──

/// offer() 入发布区 或 result(done=true) 出发布区时触发
pub const EVENT_BOARD_UPDATE: &str = "board_update";

/// HumanProvider 为人类成员创建待处理请求时触发
pub const EVENT_HUMAN_REQUEST: &str = "human_request";

/// delegate 右移 或 finish(done=true) 左移时触发
pub const EVENT_CHAIN_UPDATE: &str = "chain_update";

/// 成员开始或结束处理委托时触发
pub const EVENT_MEMBER_STATUS: &str = "member_status";

/// LLM 流式 chunk——所有成员输出都走这个类型
pub const EVENT_STREAM_CHUNK: &str = "stream_chunk";
