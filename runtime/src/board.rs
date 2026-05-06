//! 委托板（Delegation Board）
//!
//! 纯任务队列——不感知成员是否存在、忙闲。成员自己决定何时 poll。
//!
//! # 状态机
//!
//! ```text
//! published ──→ assigned ──→ completed
//!              │   │
//!              │   └──→ interrupted (cancel)
//!              └──→ rejected ──→ disputed
//! ```

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

/// 用户消息的优先级——高于一切 agent 任务
pub const PRIORITY_USER: u32 = u32::MAX;

// ── Types ────────────────────────────────────────────────────

/// 任务状态
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Published,
    Assigned,
    Completed,
    Rejected,
    Disputed,
    Interrupted,
}

/// 委托任务
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelegationTask {
    pub id: String,
    pub from: String,
    pub to: String,
    pub description: String,
    pub parameters: serde_json::Value,
    pub priority: u32,
    pub status: TaskStatus,
    pub reject_reason: Option<String>,
    pub reject_count: u32,
    pub result: Option<String>,
}

/// 委托板——纯任务队列
///
/// 两个队列：pending（待分配）和 archive（已分配/已完成/已驳回的归档）。
/// 不维护成员状态，成员自己决定何时 poll。
pub struct DelegationBoard {
    pending: RwLock<Vec<DelegationTask>>,
    archive: RwLock<Vec<DelegationTask>>,
    dispute_threshold: u32,
}

// ── Lifecycle ────────────────────────────────────────────────

impl DelegationBoard {
    pub fn new() -> Self {
        Self {
            pending: RwLock::new(Vec::new()),
            archive: RwLock::new(Vec::new()),
            dispute_threshold: 1,
        }
    }

    pub fn with_dispute_threshold(mut self, threshold: u32) -> Self {
        self.dispute_threshold = threshold;
        self
    }
}

// ── Operations ───────────────────────────────────────────────

impl DelegationBoard {
    /// 发布委托任务
    ///
    /// 任务进入 pending 队列，等待目标成员 poll。
    /// 不再校验目标是否存在——成员管理是 Workspace 的职责。
    pub async fn post(
        &self,
        from: &str,
        to: &str,
        description: &str,
        parameters: serde_json::Value,
        priority: u32,
    ) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let task = DelegationTask {
            id: id.clone(),
            from: from.to_string(),
            to: to.to_string(),
            description: description.to_string(),
            parameters,
            priority,
            status: TaskStatus::Published,
            reject_reason: None,
            reject_count: 0,
            result: None,
        };

        let mut pending = self.pending.write().await;
        pending.push(task);
        pending.sort_by(|a, b| b.priority.cmp(&a.priority));
        Ok(id)
    }

    /// 轮询分配给自己的任务
    ///
    /// 返回优先级最高、目标为自己的 Published 任务，并标记为 Assigned。
    /// 没有匹配任务时返回 `None`。
    pub async fn poll(&self, member_id: &str) -> Result<Option<DelegationTask>> {
        let mut pending = self.pending.write().await;
        if let Some(pos) = pending.iter().position(|t| t.to == member_id) {
            let mut task = pending.remove(pos);
            task.status = TaskStatus::Assigned;

            let task_for_archive = task.clone();
            let mut archive = self.archive.write().await;
            archive.push(task_for_archive);

            Ok(Some(task))
        } else {
            Ok(None)
        }
    }

    /// 取消任务
    ///
    /// Published: 从 pending 直接移除。
    /// Assigned / Rejected / Disputed: 标记 Interrupted。
    /// Completed / Interrupted: 终态，不可取消。
    pub async fn cancel(&self, task_id: &str) -> Result<()> {
        // 先在 archive 中查找
        {
            let mut archive = self.archive.write().await;
            if let Some(task) = archive.iter_mut().find(|t| t.id == task_id) {
                match task.status {
                    TaskStatus::Completed | TaskStatus::Interrupted => {
                        bail!("任务 '{}' 已处于终态 {:?}，不能取消", task_id, task.status);
                    }
                    TaskStatus::Assigned | TaskStatus::Rejected | TaskStatus::Disputed => {
                        task.status = TaskStatus::Interrupted;
                        return Ok(());
                    }
                    _ => {}
                }
            }
        }

        // 不在 archive，检查 pending
        let mut pending = self.pending.write().await;
        if let Some(pos) = pending.iter().position(|t| t.id == task_id) {
            pending.remove(pos);
            return Ok(());
        }

        bail!("任务 '{}' 不存在", task_id)
    }

    /// 完成任务
    pub async fn complete(&self, member_id: &str, task_id: &str, result: &str) -> Result<()> {
        self.transition(member_id, task_id, TaskStatus::Completed, |task| {
            task.result = Some(result.to_string());
        })
        .await
    }

    /// 驳回任务结果
    ///
    /// 驳回次数超过阈值后自动变为 Disputed。
    pub async fn reject(&self, member_id: &str, task_id: &str, reason: &str) -> Result<()> {
        let threshold = self.dispute_threshold;
        self.transition(member_id, task_id, TaskStatus::Rejected, move |task| {
            task.reject_reason = Some(reason.to_string());
            task.reject_count += 1;
            if task.reject_count > threshold {
                task.status = TaskStatus::Disputed;
            }
        })
        .await
    }

    /// 状态转换
    async fn transition<F>(
        &self,
        member_id: &str,
        task_id: &str,
        _expected_status: TaskStatus,
        mut apply: F,
    ) -> Result<()>
    where
        F: FnMut(&mut DelegationTask) + Send,
    {
        let mut archive = self.archive.write().await;
        let task = archive
            .iter_mut()
            .find(|t| t.id == task_id)
            .ok_or_else(|| anyhow::anyhow!("任务 '{}' 不存在", task_id))?;

        if task.to != member_id {
            bail!("任务 '{}' 不属于成员 '{}'", task_id, member_id);
        }
        if task.status != TaskStatus::Assigned {
            bail!(
                "任务 '{}' 当前状态为 {:?}，不能执行此操作",
                task_id,
                task.status
            );
        }

        apply(task);

        // Disputed 自动重新发布给 manager
        if task.status == TaskStatus::Disputed {
            let mut pending = self.pending.write().await;
            let mut reopened = task.clone();
            reopened.to = "manager".to_string();
            reopened.status = TaskStatus::Published;
            pending.push(reopened);
        }

        Ok(())
    }

    /// 获取指派给指定成员的当前任务
    pub async fn current_task(&self, member_id: &str) -> Option<DelegationTask> {
        let archive = self.archive.read().await;
        archive
            .iter()
            .find(|t| t.to == member_id && t.status == TaskStatus::Assigned)
            .cloned()
    }

    /// 查询委托板快照
    pub async fn status(&self) -> BoardStatus {
        let pending = self.pending.read().await;
        let archive = self.archive.read().await;
        BoardStatus {
            pending_count: pending.len(),
            archive_count: archive.len(),
        }
    }
}

/// 委托板快照
#[derive(Debug, Clone, Serialize)]
pub struct BoardStatus {
    pub pending_count: usize,
    pub archive_count: usize,
}

// ── Tests ────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn post_and_poll_flow() {
        let board = DelegationBoard::new();
        let task_id = board
            .post("architect", "coder", "write hello world", serde_json::json!({"lang": "rust"}), 1)
            .await
            .unwrap();
        assert!(!task_id.is_empty());

        let task = board.poll("coder").await.unwrap().unwrap();
        assert_eq!(task.id, task_id);
        assert_eq!(task.status, TaskStatus::Assigned);
        assert_eq!(task.from, "architect");
    }

    #[tokio::test]
    async fn complete_task() {
        let board = DelegationBoard::new();
        let task_id = board
            .post("architect", "coder", "write code", serde_json::json!({}), 1)
            .await
            .unwrap();

        board.poll("coder").await.unwrap();
        board.complete("coder", &task_id, "fn main() {}").await.unwrap();

        let status = board.status().await;
        assert_eq!(status.pending_count, 0);
    }

    #[tokio::test]
    async fn reject_and_dispute() {
        let board = DelegationBoard::new().with_dispute_threshold(1);
        let task_id = board
            .post("architect", "coder", "write code", serde_json::json!({}), 1)
            .await
            .unwrap();

        board.poll("coder").await.unwrap();
        board.reject("coder", &task_id, "not good enough").await.unwrap();

        // 超过阈值，重新发布给 manager
        let pending = board.status().await.pending_count;
        assert_eq!(pending, 1);
    }

    #[tokio::test]
    async fn priority_ordering() {
        let board = DelegationBoard::new();
        board.post("architect", "coder", "low", serde_json::json!({}), 1).await.unwrap();
        board.post("architect", "coder", "high", serde_json::json!({}), 10).await.unwrap();

        let task = board.poll("coder").await.unwrap().unwrap();
        assert_eq!(task.description, "high");
    }

    #[tokio::test]
    async fn cancel_assigned_task() {
        let board = DelegationBoard::new();
        let task_id = board
            .post("architect", "coder", "task", serde_json::json!({}), 1)
            .await
            .unwrap();

        board.poll("coder").await.unwrap();
        board.cancel(&task_id).await.unwrap();

        let status = board.status().await;
        assert_eq!(status.pending_count, 0);
    }

    #[tokio::test]
    async fn cancel_published_task() {
        let board = DelegationBoard::new();
        let task_id = board
            .post("architect", "coder", "no longer needed", serde_json::json!({}), 1)
            .await
            .unwrap();

        assert_eq!(board.status().await.pending_count, 1);
        board.cancel(&task_id).await.unwrap();
        assert_eq!(board.status().await.pending_count, 0);
    }

    #[tokio::test]
    async fn user_priority_overrides_all() {
        let board = DelegationBoard::new();
        board.post("architect", "coder", "normal", serde_json::json!({}), 5).await.unwrap();
        board.post("user", "coder", "urgent!", serde_json::json!({}), PRIORITY_USER).await.unwrap();

        let task = board.poll("coder").await.unwrap().unwrap();
        assert_eq!(task.description, "urgent!");
        assert_eq!(task.from, "user");
    }

    #[tokio::test]
    async fn cancel_nonexistent_task_fails() {
        let board = DelegationBoard::new();
        let err = board.cancel("nonexistent").await.unwrap_err();
        assert!(err.to_string().contains("不存在"));
    }
}
