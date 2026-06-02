//! Workspace 事件信道与事件类型名录
//!
//! ## 信道（channel）
//! - `CH_WORKSPACE_STREAM` — 前端长期 SSE 连接的订阅 key
//!   Board 构造时预建此通道，所有低频 workspace 事件共用
//!
//! ## 事件类型（type 字段值）
//! 以下常量对应各类事件的 `type` JSON 字段。前端收事件后据此分发，
//! 后端构造事件时引用常量而非硬编码字符串。

// ── 信道 ──

/// runtime → server 事件桥接通道 key
/// Board::trigger_event() 写入原始事件名+数据，server/event_bridge 订阅后构造完整消息
pub const CH_RAW_EVENTS: &str = "raw_events";

/// workspace 统一事件流的 board events 通道 key
/// 前端通过 GET /workspace/{name}/stream 订阅
pub const CH_WORKSPACE_STREAM: &str = "workspace_stream";

// ── 事件类型 ──

/// offer() 入发布区 或 result(done=true) 出发布区时触发
/// 事件体见 [`super::events::BoardUpdateEvent`]
pub const EVT_BOARD_UPDATE: &str = "board_update";

/// HumanProvider 为人类成员创建待处理请求时触发
/// 事件体见 [`super::events::HumanRequestEvent`]
pub const EVT_HUMAN_REQUEST: &str = "human_request";

/// delegate 右移 或 finish(done=true) 左移时触发
/// 事件体见 [`super::events::ChainUpdateEvent`]
pub const EVT_CHAIN_UPDATE: &str = "chain_update";

/// 成员开始或结束处理委托时触发
/// 事件体见 [`super::events::MemberStatusEvent`]
pub const EVT_MEMBER_STATUS: &str = "member_status";
