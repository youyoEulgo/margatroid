//! Engine — 成员调度引擎
//!
//! 职责单一：把成员、board、tools、events 串成一条流水线。
//!
//! member_loop — 死循环，等任务 → 执行 → 等任务
//! execute_task — 单次执行，读链 → match → process → 重试
//! parse_retry — 提取重试计数

use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use types::events::{EventContent, EventMetadata, WorkspaceEvent};

use crate::board::{DelegationBoard, TaskResult};
use crate::events::EventBus;
use crate::member::Agent;

const MAX_RETRIES: u32 = 3;

/// 成员控制循环
///
/// spawn 进 tokio，直到关停。
/// 不存状态、不返回——只做调度。
pub async fn member_loop(
    agent: Arc<dyn Agent>,
    board: Arc<DelegationBoard>,
    tools: Vec<types::RequestTool>,
    event_bus: Arc<EventBus>,
    workspace_name: String,
    system_prompt: String,
    member_profiles: Vec<types::MemberProfile>,
    shutdown: CancellationToken,
) {
    tracing::info!("成员 '{}' 启动控制循环", agent.id());

    loop {
        if shutdown.is_cancelled() {
            break;
        }
        execute_task(
            &*agent,
            &board,
            &tools,
            &event_bus,
            &workspace_name,
            &system_prompt,
            &member_profiles,
        )
        .await;
        tokio::select! {
            _ = board.wait(agent.id()) => {},
            _ = shutdown.cancelled() => {},
        }
    }

    tracing::info!("成员 '{}' 控制循环退出", agent.id());
}

/// 单次任务执行
///
/// 读链 → 匹配 → process → 成功则结束，失败则重试（最多 3 次）
async fn execute_task(
    agent: &dyn Agent,
    board: &DelegationBoard,
    tools: &[types::RequestTool],
    event_bus: &EventBus,
    workspace_name: &str,
    system_prompt: &str,
    member_profiles: &[types::MemberProfile],
) {
    let chain = board.chain_snapshot().await;
    let task = match chain.current_task() {
        Some(t) if t.to == agent.id() && !t.id.is_empty() => t,
        _ => return,
    };

    if has_outcome(&chain, chain.head_pos()) {
        tracing::info!("processing | {} → {} | {}", task.from, task.to, task.brief);
    } else {
        tracing::info!("delegation | {} → {} | {}", task.from, task.to, task.brief);
    }

    let task_id = task.id.clone();
    let task_from = task.from.clone();
    let task_brief = task.brief.clone();
    let task_detail = task.detail.clone();
    let parent_id = task.parent_id.clone();

    send_event(
        event_bus,
        workspace_name,
        "member_status",
        agent.id(),
        &task_id,
        &EventContent::MemberStatus {
            state: "working".into(),
        },
    );

    match agent
        .process(board, tools, system_prompt, member_profiles)
        .await
    {
        Ok(_outcome) => {
            tracing::debug!("成员 '{}' 完成委托 '{}'", agent.id(), task_id);
        }
        Err(e) => {
            // 记录失败产出
            let _ = board
                .result(
                    agent.id(),
                    TaskResult {
                        delegation_id: task_id.clone(),
                        detail: e.to_string(),
                        summary: "执行失败".into(),
                        done: false,
                        reply: String::new(),
                    },
                )
                .await;

            let (retries, inner_detail) = parse_retry(&task_detail);
            if retries >= MAX_RETRIES {
                tracing::warn!("任务 '{}' 已达最大重试次数 {}，放弃", task_id, MAX_RETRIES);
                send_event(
                    event_bus,
                    workspace_name,
                    "member_status",
                    agent.id(),
                    &task_id,
                    &EventContent::MemberStatus {
                        state: "idle".into(),
                    },
                );
                return;
            }

            let detail = format!("[RETRY:{}] {}", retries + 1, inner_detail);
            let _ = board
                .offer(
                    &task_from,
                    agent.id(),
                    &task_brief,
                    &detail,
                    parent_id.as_deref(),
                )
                .await;
        }
    }

    send_event(
        event_bus,
        workspace_name,
        "member_status",
        agent.id(),
        &task_id,
        &EventContent::MemberStatus {
            state: "idle".into(),
        },
    );
}

/// 链头委托是否已有产出
fn has_outcome(chain: &crate::board::TaskChain, head: usize) -> bool {
    chain.entries.iter().any(|e| {
        matches!(e, crate::board::ChainEntry::Outcome { delegate_idx, .. } if *delegate_idx == head)
    })
}

/// 发送事件到 workspace 统一通道
fn send_event(
    event_bus: &EventBus,
    workspace_name: &str,
    event_type: &str,
    member_id: &str,
    delegation_id: &str,
    content: &EventContent,
) {
    let payload = EventMetadata::new(event_type, member_id, delegation_id);
    let event = WorkspaceEvent {
        metadata: payload,
        content: content.clone(),
    };
    if let Ok(json) = serde_json::to_string(&event) {
        let channel = format!("{}/stream", workspace_name);
        let _ = event_bus.send(&channel, json);
    }
}

fn parse_retry(detail: &str) -> (u32, &str) {
    if let Some(rest) = detail.strip_prefix("[RETRY:") {
        if let Some(idx) = rest.find(']') {
            if let Ok(n) = rest[..idx].parse::<u32>() {
                return (n, rest[idx + 1..].trim());
            }
        }
    }
    (0, detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_retry_no_prefix() {
        let (n, detail) = parse_retry("some task detail");
        assert_eq!(n, 0);
        assert_eq!(detail, "some task detail");
    }

    #[test]
    fn parse_retry_first() {
        let (n, detail) = parse_retry("[RETRY:1] failed: timeout");
        assert_eq!(n, 1);
        assert_eq!(detail, "failed: timeout");
    }

    #[test]
    fn parse_retry_third() {
        let (n, detail) = parse_retry("[RETRY:3] something broke");
        assert_eq!(n, 3);
        assert_eq!(detail, "something broke");
    }

    #[test]
    fn parse_retry_malformed_no_bracket() {
        let (n, detail) = parse_retry("[RETRY:2");
        assert_eq!(n, 0);
        assert_eq!(detail, "[RETRY:2");
    }

    #[test]
    fn parse_retry_malformed_not_a_number() {
        let (n, detail) = parse_retry("[RETRY:two] error");
        assert_eq!(n, 0);
        assert_eq!(detail, "[RETRY:two] error");
    }
}
