//! 事件桥接 — 监听 runtime 推来的原始事件名，构造类型化消息推往前端通道。
//!
//! runtime 层通过 Board::trigger_event() 将事件名写入 raw_events 通道。
//! 此模块订阅 raw_events，根据事件名从 AppState 获取上下文，构造完整 JSON，
//! 推送至 workspace_stream 供前端 SSE 订阅。

use runtime::DelegationBoard;
use std::sync::Arc;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;
use types::{event_index, events};

/// 启动事件桥接任务：订阅 raw_events，构造消息推送 workspace_stream
pub fn spawn_event_bridge(board: Arc<DelegationBoard>) {
    tokio::spawn(async move {
        let rx = match board.register_listener(event_index::CH_RAW_EVENTS).await {
            Some(rx) => rx,
            None => return,
        };

        let mut stream = BroadcastStream::new(rx);
        while let Some(Ok(payload)) = stream.next().await {
            // payload: "event_name" 或 "event_name\ndata"
            let (event_name, data) = payload
                .split_once('\n')
                .map_or((payload.as_str(), ""), |(n, d)| (n, d));

            let json = match event_name {
                event_index::EVENT_BOARD_UPDATE => {
                    let count: usize = data.parse().unwrap_or(0);
                    serde_json::to_string(&events::BoardUpdateEvent::new(count)).unwrap_or_default()
                }
                event_index::EVENT_CHAIN_UPDATE => {
                    let chain = board.chain_snapshot().await;
                    let task = chain
                        .current_task()
                        .map(|t| (t.from.clone(), t.to.clone(), t.brief.clone()));
                    let head_pos = chain.head_pos();
                    serde_json::to_string(&events::ChainUpdateEvent::new(task, head_pos))
                        .unwrap_or_default()
                }
                event_index::EVENT_MEMBER_STATUS => {
                    let parts: Vec<&str> = data.splitn(2, '\n').collect();
                    let member_id = parts.first().copied().unwrap_or("").to_string();
                    let state = parts.get(1).copied().unwrap_or("").to_string();
                    serde_json::to_string(&events::MemberStatusEvent::new(member_id, state))
                        .unwrap_or_default()
                }
                _ => continue,
            };

            board
                .publish_raw(event_index::CH_WORKSPACE_STREAM, &json)
                .await;
        }
    });
}
