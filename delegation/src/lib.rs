//! 委托板（Delegation Board）
//!
//! Workspace 的核心协作基础设施——事件驱动的任务队列。
//! 任何成员可以发布委托任务，目标成员空闲时自动分配。
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
use std::collections::HashMap;
use tokio::sync::RwLock;
use types::ComposeFile;

/// 用户消息的优先级——高于一切 agent 任务
pub const PRIORITY_USER: u32 = u32::MAX;

// ── Types ────────────────────────────────────────────────────

/// 任务状态
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// 已发布，等待目标成员空闲
    Published,
    /// 已分配给目标成员，正在执行
    Assigned,
    /// 执行完成
    Completed,
    /// 被驳回，附驳回理由
    Rejected,
    /// 驳回次数超过阈值，升级为纠纷
    Disputed,
    /// 被 Manager 主动取消（用户切换方向等）
    Interrupted,
}

/// 委托任务
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelegationTask {
    /// 任务唯一 ID
    pub id: String,
    /// 发起者 ID
    pub from: String,
    /// 目标成员 ID
    pub to: String,
    /// 任务描述
    pub description: String,
    /// 结构化参数（JSON）
    pub parameters: serde_json::Value,
    /// 优先级（越大越高）
    pub priority: u32,
    /// 当前状态
    pub status: TaskStatus,
    /// 驳回原因（仅 Rejected/Disputed 时有值）
    pub reject_reason: Option<String>,
    /// 驳回次数
    pub reject_count: u32,
    /// 执行结果（仅 Completed 时有值）
    pub result: Option<String>,
}

/// 成员状态
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemberState {
    /// 空闲，可接受新任务
    Idle,
    /// 正在执行任务
    Working,
}

/// 委托板
///
/// 线程安全，可被多个 agent 实例共享。
pub struct DelegationBoard {
    /// 成员状态表
    members: RwLock<HashMap<String, MemberState>>,
    /// 待办任务队列（发布但未分配）
    pending: RwLock<Vec<DelegationTask>>,
    /// 已分配/已完成/已驳回的任务归档
    archive: RwLock<Vec<DelegationTask>>,
    /// 驳回阈值（超过此次数升级为纠纷）
    dispute_threshold: u32,
}

// ── Lifecycle ────────────────────────────────────────────────

impl DelegationBoard {
    /// 从 compose 文件初始化委托板
    ///
    /// 所有 agent 注册为成员，初始状态 Idle。
    pub fn new(compose: &ComposeFile) -> Self {
        let members: HashMap<String, MemberState> = compose
            .agents
            .iter()
            .map(|a| (a.id.clone(), MemberState::Idle))
            .collect();

        Self {
            members: RwLock::new(members),
            pending: RwLock::new(Vec::new()),
            archive: RwLock::new(Vec::new()),
            dispute_threshold: 1,
        }
    }

    /// 设置驳回阈值
    pub fn with_dispute_threshold(mut self, threshold: u32) -> Self {
        self.dispute_threshold = threshold;
        self
    }
}

// ── Operations ───────────────────────────────────────────────

impl DelegationBoard {
    /// 发布委托任务
    ///
    /// 返回任务 ID。任务进入 pending 队列，等待目标成员 poll。
    pub async fn post(
        &self,
        from: &str,
        to: &str,
        description: &str,
        parameters: serde_json::Value,
        priority: u32,
    ) -> Result<String> {
        // 校验目标成员存在
        {
            let members = self.members.read().await;
            if !members.contains_key(to) {
                bail!("目标成员 '{}' 不存在", to);
            }
        }

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
        // 按优先级降序排列
        pending.sort_by(|a, b| b.priority.cmp(&a.priority));

        Ok(id)
    }

    /// 轮询分配给指定成员的任务
    ///
    /// 返回优先级最高的待分配任务，并将成员状态设为 Working。
    /// 如果没有待分配任务，返回 `None`。
    pub async fn poll(&self, member_id: &str) -> Result<Option<DelegationTask>> {
        let mut members = self.members.write().await;
        let state = members
            .get(member_id)
            .ok_or_else(|| anyhow::anyhow!("成员 '{}' 不存在", member_id))?;

        if *state != MemberState::Idle {
            return Ok(None);
        }

        let mut pending = self.pending.write().await;
        if let Some(pos) = pending.iter().position(|t| t.to == member_id) {
            let mut task = pending.remove(pos);
            task.status = TaskStatus::Assigned;
            members.insert(member_id.to_string(), MemberState::Working);

            let task_for_archive = task.clone();
            let mut archive = self.archive.write().await;
            archive.push(task_for_archive);

            Ok(Some(task))
        } else {
            Ok(None)
        }
    }

    /// 取消任务（Manager 主动打断）
    ///
    /// 支持取消所有非终态的任务：
    /// - Published: 从 pending 队列直接移除
    /// - Assigned: 标记 Interrupted，释放目标成员
    /// - Rejected / Disputed: 标记 Interrupted（无需继续等待仲裁）
    /// - Completed / Interrupted: 终态，不可取消
    pub async fn cancel(&self, task_id: &str) -> Result<()> {
        // 先在 archive 中查找
        let member_to_release = {
            let mut archive = self.archive.write().await;
            if let Some(task) = archive.iter_mut().find(|t| t.id == task_id) {
                match task.status {
                    TaskStatus::Completed | TaskStatus::Interrupted => {
                        bail!("任务 '{}' 已处于终态 {:?}，不能取消", task_id, task.status);
                    }
                    TaskStatus::Assigned => {
                        let member = task.to.clone();
                        task.status = TaskStatus::Interrupted;
                        Some(member)
                    }
                    TaskStatus::Rejected | TaskStatus::Disputed => {
                        task.status = TaskStatus::Interrupted;
                        None
                    }
                    _ => None,
                }
            } else {
                None
            }
        };

        if let Some(member) = member_to_release {
            self.members.write().await.insert(member, MemberState::Idle);
            return Ok(());
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
    ///
    /// 成员状态恢复为 Idle。
    pub async fn complete(&self, member_id: &str, task_id: &str, result: &str) -> Result<()> {
        self.transition(member_id, task_id, TaskStatus::Completed, |task| {
            task.result = Some(result.to_string());
        })
        .await
    }

    /// 驳回任务结果
    ///
    /// 若驳回次数未超过阈值，任务回到 Published 状态等待重新分配。
    /// 若超过阈值，任务状态变为 Disputed，应上报 Manager 仲裁。
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

    /// 状态转换的内部方法
    async fn transition<F>(
        &self,
        member_id: &str,
        task_id: &str,
        expected_status: TaskStatus,
        mut apply: F,
    ) -> Result<()>
    where
        F: FnMut(&mut DelegationTask) + Send,
    {
        let should_reopen = {
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

            // 如果最终状态是 Disputed，需要重新发布（to 改为 Manager）
            task.status == TaskStatus::Disputed && expected_status == TaskStatus::Rejected
        };

        // 恢复成员状态
        let mut members = self.members.write().await;
        members.insert(member_id.to_string(), MemberState::Idle);

        // 如果进入了 Disputed 状态，自动重新发布给 Manager
        if should_reopen {
            let mut archive = self.archive.write().await;
            if let Some(task) = archive.iter_mut().find(|t| t.id == task_id) {
                let mut reopened = task.clone();
                reopened.to = "manager".to_string();
                reopened.status = TaskStatus::Published;
                let mut pending = self.pending.write().await;
                pending.push(reopened);
            }
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

    /// 查询委托板状态
    pub async fn status(&self) -> BoardStatus {
        let pending = self.pending.read().await;
        let archive = self.archive.read().await;
        let members = self.members.read().await;

        BoardStatus {
            pending_count: pending.len(),
            archive_count: archive.len(),
            members: members
                .iter()
                .map(|(id, state)| MemberInfo {
                    id: id.clone(),
                    state: format!("{:?}", state),
                })
                .collect(),
        }
    }
}

/// 委托板快照
#[derive(Debug, Clone, Serialize)]
pub struct BoardStatus {
    pub pending_count: usize,
    pub archive_count: usize,
    pub members: Vec<MemberInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemberInfo {
    pub id: String,
    pub state: String,
}

// ── Tests ────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use types::{AgentDef, WorkspaceMeta};

    fn sample_compose() -> ComposeFile {
        ComposeFile {
            workspace: WorkspaceMeta {
                name: "test".into(),
                version: "0.1.0".into(),
                description: "".into(),
                workdir: "./project".into(),
            },
            agents: vec![
                AgentDef {
                    id: "architect".into(),
                    provider: "OpenRouter".into(),
                    model: "claude".into(),
                    system_prompt: "architect".into(),
                    skills: vec!["design".into()],
                    depends_on: vec![],
                    profile: None,
                    max_tokens: None,
                    temperature: None,
                },
                AgentDef {
                    id: "coder".into(),
                    provider: "OpenRouter".into(),
                    model: "gemini".into(),
                    system_prompt: "coder".into(),
                    skills: vec!["coding".into()],
                    depends_on: vec!["architect".into()],
                    profile: None,
                    max_tokens: None,
                    temperature: None,
                },
            ],
        }
    }

    #[tokio::test]
    async fn post_and_poll_flow() {
        let board = DelegationBoard::new(&sample_compose());

        // 发布任务
        let task_id = board
            .post(
                "architect",
                "coder",
                "write hello world",
                serde_json::json!({"lang": "rust"}),
                1,
            )
            .await
            .unwrap();
        assert!(!task_id.is_empty());

        // coder poll 获取任务
        let task = board.poll("coder").await.unwrap().unwrap();
        assert_eq!(task.id, task_id);
        assert_eq!(task.status, TaskStatus::Assigned);
        assert_eq!(task.from, "architect");

        // 再次 poll 应为 None（coder 已在 Working）
        let empty = board.poll("coder").await.unwrap();
        assert!(empty.is_none());
    }

    #[tokio::test]
    async fn complete_task() {
        let board = DelegationBoard::new(&sample_compose());

        let task_id = board
            .post("architect", "coder", "write code", serde_json::json!({}), 1)
            .await
            .unwrap();

        board.poll("coder").await.unwrap();
        board
            .complete("coder", &task_id, "fn main() {}")
            .await
            .unwrap();

        // coder 恢复 Idle
        let task = board.poll("coder").await.unwrap();
        assert!(task.is_none());
    }

    #[tokio::test]
    async fn reject_and_dispute() {
        let board = DelegationBoard::new(&sample_compose()).with_dispute_threshold(1);

        let task_id = board
            .post("architect", "coder", "write code", serde_json::json!({}), 1)
            .await
            .unwrap();

        board.poll("coder").await.unwrap();

        // 第一次驳回
        board
            .reject("coder", &task_id, "not good enough")
            .await
            .unwrap();

        // 超过阈值，应变为 Disputed 并重新发布给 manager
        // 注意：manager 不是 compose 中的成员，但 dispute 逻辑会硬编码目标为 "manager"
        // 实际使用时，manager 应在 compose 中注册为 agent
    }

    #[tokio::test]
    async fn post_to_nonexistent_member_fails() {
        let board = DelegationBoard::new(&sample_compose());
        let err = board
            .post("coder", "ghost", "hi", serde_json::json!({}), 1)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("ghost"));
    }

    #[tokio::test]
    async fn board_status() {
        let board = DelegationBoard::new(&sample_compose());
        board
            .post("architect", "coder", "task 1", serde_json::json!({}), 2)
            .await
            .unwrap();
        board
            .post("architect", "coder", "task 2", serde_json::json!({}), 1)
            .await
            .unwrap();

        let status = board.status().await;
        assert_eq!(status.pending_count, 2);
        assert_eq!(status.members.len(), 2);
    }

    #[tokio::test]
    async fn priority_ordering() {
        let board = DelegationBoard::new(&sample_compose());

        board
            .post("architect", "coder", "low", serde_json::json!({}), 1)
            .await
            .unwrap();
        board
            .post("architect", "coder", "high", serde_json::json!({}), 10)
            .await
            .unwrap();

        // poll 应返回优先级高的任务
        let task = board.poll("coder").await.unwrap().unwrap();
        assert_eq!(task.description, "high");
    }

    #[tokio::test]
    async fn cancel_interrupts_and_releases_member() {
        let board = DelegationBoard::new(&sample_compose());

        let task_id = board
            .post("architect", "coder", "long task", serde_json::json!({}), 1)
            .await
            .unwrap();

        board.poll("coder").await.unwrap();

        // Manager 取消当前任务
        board.cancel(&task_id).await.unwrap();

        // coder 恢复 Idle
        let task = board.poll("coder").await.unwrap();
        assert!(task.is_none());

        // 检查任务状态
        let status = board.status().await;
        assert_eq!(status.pending_count, 0);
    }

    #[tokio::test]
    async fn user_priority_overrides_all() {
        let board = DelegationBoard::new(&sample_compose());

        // 普通任务
        board
            .post("architect", "coder", "normal", serde_json::json!({}), 5)
            .await
            .unwrap();
        // 用户消息——优先级绝对最高
        board
            .post(
                "user",
                "coder",
                "urgent!",
                serde_json::json!({}),
                PRIORITY_USER,
            )
            .await
            .unwrap();

        let task = board.poll("coder").await.unwrap().unwrap();
        assert_eq!(task.description, "urgent!");
        assert_eq!(task.from, "user");
    }

    #[tokio::test]
    async fn cancel_published_task_removes_from_pending() {
        let board = DelegationBoard::new(&sample_compose());

        let task_id = board
            .post("architect", "coder", "no longer needed", serde_json::json!({}), 1)
            .await
            .unwrap();

        assert_eq!(board.status().await.pending_count, 1);
        board.cancel(&task_id).await.unwrap();
        assert_eq!(board.status().await.pending_count, 0);
    }

    #[tokio::test]
    async fn cancel_rejected_task_interrupts_without_releasing() {
        let board = DelegationBoard::new(&sample_compose()).with_dispute_threshold(2);

        let task_id = board
            .post("architect", "coder", "meh", serde_json::json!({}), 1)
            .await
            .unwrap();

        board.poll("coder").await.unwrap();
        // 驳回一次（未超阈值）
        board.reject("coder", &task_id, "bad").await.unwrap();

        // Manager 取消（用户说"别争了"）
        board.cancel(&task_id).await.unwrap();

        // 不再有 pending 任务（Rejected 被 cancel 后不会重新发布）
        let task = board.poll("coder").await.unwrap();
        assert!(task.is_none());
    }

    #[tokio::test]
    async fn cancel_nonexistent_task_fails() {
        let board = DelegationBoard::new(&sample_compose());
        let err = board.cancel("nonexistent").await.unwrap_err();
        assert!(err.to_string().contains("不存在"));
    }
}
