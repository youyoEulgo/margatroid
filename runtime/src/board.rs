//! 委托板（Delegation Board）
//!
//! 四区模型：发布区 → 执行区 → 返回区 → 档案区（SQLite）。
//!
//! ```text
//! offer ──→ [发布区] ──claim──→ [执行区] ──return──→ [返回区]
//!               ↑                                    │       │
//!               └────────── reject ──────────────────┘       │
//!                                                            │
//!                                    accept → [档案区: SQLite]
//! ```

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

use crate::memory::{MemoryEntry, PersonalMemory, SqliteMemory, Worklog, WorklogEntry};

/// 用户消息的优先级——高于一切 agent 任务
pub const PRIORITY_USER: u32 = u32::MAX;

/// 纠纷升级阈值
const DISPUTE_THRESHOLD: u32 = 3;

// ── Types ────────────────────────────────────────────────────

/// 委托任务
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelegationTask {
    pub id: String,
    pub from: String,
    pub to: String,
    pub description: String,
    pub parameters: serde_json::Value,
    pub priority: u32,
    pub result: Option<String>,
    pub reject_count: u32,
}

impl DelegationTask {
    fn new(
        from: &str,
        to: &str,
        description: &str,
        parameters: serde_json::Value,
        priority: u32,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            from: from.to_string(),
            to: to.to_string(),
            description: description.to_string(),
            parameters,
            priority,
            result: None,
            reject_count: 0,
        }
    }

    /// 克隆并重置为可重新发布的状态
    fn clone_for_reoffer(&self) -> Self {
        Self {
            id: self.id.clone(),
            from: self.from.clone(),
            to: self.to.clone(),
            description: self.description.clone(),
            parameters: self.parameters.clone(),
            priority: self.priority,
            result: None,
            reject_count: self.reject_count,
        }
    }
}

/// 委托板——三区任务队列
pub struct DelegationBoard {
    publish: RwLock<Vec<DelegationTask>>,
    exec: RwLock<Vec<DelegationTask>>,
    returned: RwLock<Vec<DelegationTask>>,
    db: Arc<SqliteMemory>,
}

// ── Lifecycle ────────────────────────────────────────────────

impl DelegationBoard {
    pub fn new(db: Arc<SqliteMemory>) -> Self {
        Self {
            publish: RwLock::new(Vec::new()),
            exec: RwLock::new(Vec::new()),
            returned: RwLock::new(Vec::new()),
            db,
        }
    }
}

// ── Operations ───────────────────────────────────────────────

impl DelegationBoard {
    /// 发布委托到发布区（非阻塞，始终异步）
    pub async fn offer(
        &self,
        from: &str,
        to: &str,
        description: &str,
        parameters: serde_json::Value,
        priority: u32,
    ) -> Result<String> {
        let task = DelegationTask::new(from, to, description, parameters, priority);
        let id = task.id.clone();

        let mut publish = self.publish.write().await;
        publish.push(task);
        publish.sort_by(|a, b| b.priority.cmp(&a.priority));
        Ok(id)
    }

    /// 成员领取任务：从发布区移到执行区
    pub async fn claim(&self, member_id: &str) -> Result<Option<DelegationTask>> {
        let mut publish = self.publish.write().await;
        if let Some(pos) = publish.iter().position(|t| t.to == member_id) {
            let task = publish.remove(pos);

            let mut exec = self.exec.write().await;
            exec.push(task.clone());

            Ok(Some(task))
        } else {
            Ok(None)
        }
    }

    /// 成员提交结果：从执行区移到返回区，写入档案
    pub async fn return_task(
        &self,
        member_id: &str,
        task_id: &str,
        result: &str,
        summary: &str,
    ) -> Result<()> {
        let mut exec = self.exec.write().await;
        let pos = exec
            .iter()
            .position(|t| t.id == task_id && t.to == member_id)
            .ok_or_else(|| anyhow::anyhow!("执行区中未找到任务 '{}'", task_id))?;

        let mut task = exec.remove(pos);
        task.result = Some(result.to_string());

        let returned_task = task.clone();
        let mut returned = self.returned.write().await;
        returned.push(task);

        // 写入档案（SQLite）
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let _ = self.db.append(WorklogEntry {
            timestamp: now,
            agent_id: member_id.to_string(),
            delegation_id: returned_task.id.clone(),
            summary: format!("{} — {}", returned_task.description, summary),
            artifacts: vec![],
        });
        let _ = self.db.remember(MemoryEntry {
            timestamp: now,
            delegation_id: returned_task.id.clone(),
            description: returned_task.description.clone(),
            summary: summary.to_string(),
            artifacts: vec![],
            tags: vec![],
        });
        Ok(())
    }

    /// 发布者接受结果：从返回区删除
    ///
    /// 档案写入在 return_task 时已完成，accept 仅移除出返回区。
    pub async fn accept(&self, member_id: &str, task_id: &str) -> Result<DelegationTask> {
        let mut returned = self.returned.write().await;
        let pos = returned
            .iter()
            .position(|t| t.id == task_id && t.from == member_id)
            .ok_or_else(|| anyhow::anyhow!("返回区中未找到任务 '{}'", task_id))?;

        Ok(returned.remove(pos))
    }

    /// 发布者驳回结果：从返回区回到发布区（reject_count++）
    ///
    /// 若 reject_count 超过阈值，to 改为 manager。
    pub async fn reject(&self, member_id: &str, task_id: &str) -> Result<()> {
        let mut returned = self.returned.write().await;
        let pos = returned
            .iter()
            .position(|t| t.id == task_id && t.from == member_id)
            .ok_or_else(|| anyhow::anyhow!("返回区中未找到任务 '{}'", task_id))?;

        let task = &mut returned[pos];
        task.reject_count += 1;
        task.result = None;

        if task.reject_count >= DISPUTE_THRESHOLD {
            task.to = "manager".to_string();
        }

        let reoffer = task.clone_for_reoffer();
        returned.remove(pos);
        drop(returned);

        let mut publish = self.publish.write().await;
        publish.push(reoffer);
        publish.sort_by(|a, b| b.priority.cmp(&a.priority));
        Ok(())
    }

    /// 取消任务：从任意区删除
    pub async fn cancel(&self, task_id: &str) -> Result<DelegationTask> {
        // 执行区
        {
            let mut exec = self.exec.write().await;
            if let Some(pos) = exec.iter().position(|t| t.id == task_id) {
                return Ok(exec.remove(pos));
            }
        }
        // 发布区
        {
            let mut publish = self.publish.write().await;
            if let Some(pos) = publish.iter().position(|t| t.id == task_id) {
                return Ok(publish.remove(pos));
            }
        }
        // 返回区
        {
            let mut returned = self.returned.write().await;
            if let Some(pos) = returned.iter().position(|t| t.id == task_id) {
                return Ok(returned.remove(pos));
            }
        }
        bail!("任务 '{}' 不存在", task_id)
    }

    /// 查询返回区中自己发布的委托（等待 accept/reject）
    pub async fn check_return(&self, member_id: &str) -> Vec<DelegationTask> {
        let returned = self.returned.read().await;
        returned
            .iter()
            .filter(|t| t.from == member_id)
            .cloned()
            .collect()
    }

    /// 查询自己的任务在执行区中是否存在
    pub async fn is_working(&self, member_id: &str) -> bool {
        let exec = self.exec.read().await;
        exec.iter().any(|t| t.to == member_id)
    }

    /// 查询该成员是否有未完成的阶段任务（status='offered'）
    pub fn has_offered_schedule(&self, member_id: &str) -> bool {
        self.db.has_offered_schedule(member_id)
    }

    /// 查询快照
    pub async fn status(&self) -> BoardStatus {
        let publish = self.publish.read().await;
        let exec = self.exec.read().await;
        let returned = self.returned.read().await;
        BoardStatus {
            publish_count: publish.len(),
            exec_count: exec.len(),
            returned_count: returned.len(),
        }
    }

    // ── Schedule 委派 ──────────────────────────────────────────

    pub fn schedule_add(&self, target: &str, description: &str, priority: i32) -> Result<i64> {
        self.db.schedule_add(target, description, priority)
    }

    pub fn schedule_list(&self) -> Vec<crate::memory::ScheduleEntry> {
        self.db.schedule_list()
    }

    pub fn schedule_pop(&self, target: &str) -> Option<crate::memory::ScheduleEntry> {
        self.db.schedule_pop(target)
    }

    pub fn schedule_archive(&self, id: i64) {
        self.db.schedule_archive(id)
    }

    pub fn schedule_archive_by_target(&self, target: &str) {
        self.db.schedule_archive_by_target(target)
    }

    pub fn schedule_revert(&self, id: i64) {
        self.db.schedule_revert(id)
    }

    pub fn schedule_remove(&self, id: i64) -> Result<()> {
        self.db.schedule_remove(id)
    }

    pub fn schedule_reorder(&self, id: i64, new_priority: i32) -> Result<()> {
        self.db.schedule_reorder(id, new_priority)
    }
}

/// 委托板快照
#[derive(Debug, Clone, Serialize)]
pub struct BoardStatus {
    pub publish_count: usize,
    pub exec_count: usize,
    pub returned_count: usize,
}

// ── Tests ────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Arc<SqliteMemory> {
        Arc::new(SqliteMemory::open(":memory:").unwrap())
    }

    #[tokio::test]
    async fn offer_claim_return_accept_flow() {
        let board = DelegationBoard::new(test_db());

        let task_id = board
            .offer("architect", "coder", "write code", serde_json::json!({}), 1)
            .await
            .unwrap();
        assert!(!task_id.is_empty());
        assert_eq!(board.status().await.publish_count, 1);

        // claim
        let task = board.claim("coder").await.unwrap().unwrap();
        assert_eq!(task.id, task_id);
        assert_eq!(board.status().await.publish_count, 0);
        assert_eq!(board.status().await.exec_count, 1);

        // return
        board
            .return_task("coder", &task_id, "fn main() {}", "coder's summary")
            .await
            .unwrap();
        assert_eq!(board.status().await.exec_count, 0);
        assert_eq!(board.status().await.returned_count, 1);

        // accept
        let done = board.accept("architect", &task_id).await.unwrap();
        assert_eq!(done.result.unwrap(), "fn main() {}");
        assert_eq!(board.status().await.returned_count, 0);
    }

    #[tokio::test]
    async fn reject_goes_back_to_publish() {
        let board = DelegationBoard::new(test_db());

        let task_id = board
            .offer("architect", "coder", "write code", serde_json::json!({}), 1)
            .await
            .unwrap();

        board.claim("coder").await.unwrap();
        board
            .return_task("coder", &task_id, "bad code", "bad")
            .await
            .unwrap();
        assert_eq!(board.status().await.returned_count, 1);

        board.reject("architect", &task_id).await.unwrap();
        assert_eq!(board.status().await.returned_count, 0);
        assert_eq!(board.status().await.publish_count, 1);

        // 任务回到发布区，coder 可以重新 claim
        let retry = board.claim("coder").await.unwrap().unwrap();
        assert_eq!(retry.id, task_id);
        assert_eq!(retry.reject_count, 1);
    }

    #[tokio::test]
    async fn reject_over_threshold_escalates_to_manager() {
        let board = DelegationBoard::new(test_db());

        let task_id = board
            .offer("architect", "coder", "write code", serde_json::json!({}), 1)
            .await
            .unwrap();

        for _ in 0..DISPUTE_THRESHOLD {
            board.claim("coder").await.unwrap();
            board
                .return_task("coder", &task_id, "nope", "no")
                .await
                .unwrap();
            board.reject("architect", &task_id).await.unwrap();
        }

        // 超过阈值，to 改为 manager
        let task = board.claim("manager").await.unwrap().unwrap();
        assert_eq!(task.id, task_id);
        assert_eq!(task.to, "manager");
        assert_eq!(task.reject_count, DISPUTE_THRESHOLD);
    }

    #[tokio::test]
    async fn priority_ordering() {
        let board = DelegationBoard::new(test_db());
        board
            .offer("architect", "coder", "low", serde_json::json!({}), 1)
            .await
            .unwrap();
        board
            .offer("architect", "coder", "high", serde_json::json!({}), 10)
            .await
            .unwrap();

        let task = board.claim("coder").await.unwrap().unwrap();
        assert_eq!(task.description, "high");
    }

    #[tokio::test]
    async fn check_return_lists_pending_review() {
        let board = DelegationBoard::new(test_db());
        let task_id = board
            .offer("architect", "coder", "write code", serde_json::json!({}), 1)
            .await
            .unwrap();

        board.claim("coder").await.unwrap();
        board
            .return_task("coder", &task_id, "done", "ok")
            .await
            .unwrap();

        let pending = board.check_return("architect").await;
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, task_id);

        // coder 看不到（是发给 architect 的）
        let empty = board.check_return("coder").await;
        assert!(empty.is_empty());
    }

    #[tokio::test]
    async fn cancel_from_exec() {
        let board = DelegationBoard::new(test_db());
        let task_id = board
            .offer("architect", "coder", "write code", serde_json::json!({}), 1)
            .await
            .unwrap();

        board.claim("coder").await.unwrap();
        board.cancel(&task_id).await.unwrap();
        assert_eq!(board.status().await.exec_count, 0);
    }

    #[tokio::test]
    async fn cancel_from_publish() {
        let board = DelegationBoard::new(test_db());
        let task_id = board
            .offer(
                "architect",
                "coder",
                "no longer needed",
                serde_json::json!({}),
                1,
            )
            .await
            .unwrap();

        assert_eq!(board.status().await.publish_count, 1);
        board.cancel(&task_id).await.unwrap();
        assert_eq!(board.status().await.publish_count, 0);
    }

    #[tokio::test]
    async fn cancel_nonexistent_fails() {
        let board = DelegationBoard::new(test_db());
        let err = board.cancel("nonexistent").await.unwrap_err();
        assert!(err.to_string().contains("不存在"));
    }

    #[tokio::test]
    async fn user_priority() {
        let board = DelegationBoard::new(test_db());
        board
            .offer("architect", "coder", "normal", serde_json::json!({}), 5)
            .await
            .unwrap();
        board
            .offer(
                "user",
                "coder",
                "urgent!",
                serde_json::json!({}),
                PRIORITY_USER,
            )
            .await
            .unwrap();

        let task = board.claim("coder").await.unwrap().unwrap();
        assert_eq!(task.description, "urgent!");
    }

    #[tokio::test]
    async fn is_working_tracks_exec_zone() {
        let board = DelegationBoard::new(test_db());
        assert!(!board.is_working("coder").await);

        board
            .offer("architect", "coder", "write code", serde_json::json!({}), 1)
            .await
            .unwrap();
        board.claim("coder").await.unwrap();
        assert!(board.is_working("coder").await);

        // 另一个成员不受影响
        assert!(!board.is_working("designer").await);
    }
}
