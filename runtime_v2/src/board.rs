//! DelegationBoard V2 — 纯调度层
//!
//! 职责：
//! - 委托链管理（TaskChain：右移/左移）
//! - 发布区缓存（offer → publish，take → 移除）
//! - 成员通知（Notify 机制）
//!
//! 迁出的内容：
//! - 事件发射 → Kernel.event_bus
//! - 提示词组装 → context.rs（未来）
//! - 成员管理 → Workspace

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{Notify, RwLock};

// 暂时从旧 runtime 借用 SqliteMemory，后续可能也需要复制到 v2
use crate::memory::SqliteMemory;

// ── 任务链类型 ────────────────────────────────────────────────

/// 委托任务（纯数据）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelegationTask {
    pub id: String,
    pub from: String,
    pub to: String,
    pub brief: String,
    pub detail: String,
    pub parent_id: Option<String>,
}

impl DelegationTask {
    fn new(from: &str, to: &str, brief: &str, detail: &str, parent_id: Option<&str>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            from: from.to_string(),
            to: to.to_string(),
            brief: brief.to_string(),
            detail: detail.to_string(),
            parent_id: parent_id.map(|s| s.to_string()),
        }
    }
}

/// 委托产出（与数据库字段对应）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    pub delegation_id: String,
    pub detail: String,
    pub summary: String,
    pub done: bool,
    /// LLM 返回的对话文本（可与工具调用同时出现）
    #[serde(default)]
    pub reply: String,
}

/// 链条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ChainEntry {
    /// 一次委托
    Delegate {
        task: DelegationTask,
        parent_idx: usize,
    },
    /// 一次产出（一个委托可以有多个 Outcome）
    Outcome {
        result: TaskResult,
        delegate_idx: usize,
    },
}

/// 任务链
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskChain {
    pub entries: Vec<ChainEntry>,
    pub head: usize,
}

impl TaskChain {
    fn new(root: DelegationTask) -> Self {
        Self {
            entries: vec![ChainEntry::Delegate {
                task: root,
                parent_idx: 0,
            }],
            head: 0,
        }
    }

    fn add_task(&mut self, task: DelegationTask, db: &SqliteMemory) {
        let parent_idx = self.head;
        self.entries.push(ChainEntry::Delegate { task, parent_idx });
        self.head = self.entries.len() - 1;
        if let Some(ChainEntry::Delegate { task, .. }) = self.entries.last() {
            let _ = self.worklog_record(self.entries.last().unwrap(), db);
            let _ = db.memory_add_task(&task.id, &task.to, &task.from, &task.brief);
        }
    }

    fn add_result(&mut self, result: TaskResult, db: &SqliteMemory) {
        let delegate_idx = self.head;
        let done = result.done;
        self.entries.push(ChainEntry::Outcome {
            delegate_idx,
            result,
        });
        if let Some(ChainEntry::Outcome { result, .. }) = self.entries.last() {
            let _ = self.worklog_record(self.entries.last().unwrap(), db);
            let _ = db.memory_add_detail(&result.delegation_id, &result.detail);
        }
        if done {
            if let Some(ChainEntry::Delegate { parent_idx, .. }) = self.entries.get(self.head) {
                self.head = *parent_idx;
            }
        }
    }

    pub fn current_task(&self) -> Option<&DelegationTask> {
        match self.entries.get(self.head) {
            Some(ChainEntry::Delegate { task, .. }) => Some(task),
            _ => None,
        }
    }

    pub fn head_pos(&self) -> usize {
        self.head
    }

    fn worklog_record(&self, entry: &ChainEntry, db: &SqliteMemory) -> Result<()> {
        match entry {
            ChainEntry::Delegate { task, .. } => {
                db.worklog_add_task(&task.id, &task.from, &task.to, &task.brief)
            }
            ChainEntry::Outcome { result, .. } => {
                db.worklog_add_result(&result.delegation_id, &result.summary, &result.reply)
            }
        }
    }
}

// ── Board ───────────────────────────────────────────────

/// Board 状态（用于 status API）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoardStatus {
    pub publish_count: usize,
    pub head_pos: usize,
    pub current_task: Option<DelegationTask>,
}

/// 委托板 V2 — 纯调度层
pub struct DelegationBoard {
    /// 发布区
    publish: RwLock<Vec<DelegationTask>>,
    /// 任务链
    chain: RwLock<TaskChain>,
    /// SQLite 存储
    db: Arc<SqliteMemory>,
    /// 成员 ID 集合（用于校验）
    member_ids: RwLock<HashSet<String>>,
    /// 成员唤醒信号
    notifies: RwLock<HashMap<String, Arc<Notify>>>,
}

impl DelegationBoard {
    /// 创建新的 DelegationBoard
    pub fn new(db: Arc<SqliteMemory>, member_ids: Vec<String>) -> Self {
        Self {
            publish: RwLock::new(Vec::new()),
            chain: RwLock::new(TaskChain::new(DelegationTask {
                id: String::new(),
                from: String::new(),
                to: String::new(),
                brief: String::new(),
                detail: String::new(),
                parent_id: None,
            })),
            db,
            member_ids: RwLock::new(member_ids.into_iter().collect()),
            notifies: RwLock::new(HashMap::new()),
        }
    }

    /// 获取 SqliteMemory 引用
    pub fn db(&self) -> &SqliteMemory {
        &self.db
    }

    /// 发布新任务到委托板
    pub async fn offer(
        &self,
        from: &str,
        to: &str,
        brief: &str,
        detail: &str,
        parent_id: Option<&str>,
    ) -> Result<String> {
        // 校验目标成员合法性
        {
            let member_ids = self.member_ids.read().await;
            if !member_ids.contains(to) {
                bail!("target member '{}' not found", to);
            }
        }

        let task = DelegationTask::new(from, to, brief, detail, parent_id);
        let task_id = task.id.clone();

        // 追加到链
        {
            let mut chain = self.chain.write().await;
            chain.add_task(task.clone(), &self.db);
        }

        // 追加到发布区
        {
            let mut publish = self.publish.write().await;
            publish.push(task);
        }

        // 唤醒目标成员
        self.notify_member(to).await;

        Ok(task_id)
    }

    /// 记录任务结果
    pub async fn result(&self, _member_id: &str, result: TaskResult) -> Result<()> {
        let done = result.done;

        // 追加 Outcome 到链
        {
            let mut chain = self.chain.write().await;
            chain.add_result(result.clone(), &self.db);
        }

        // 如果 done=true，从发布区移除该任务
        if done {
            let mut publish = self.publish.write().await;
            publish.retain(|t| t.id != result.delegation_id);

            // 唤醒链头指向的成员（可能是父任务的负责人）
            if let Some(task) = self.chain.read().await.current_task() {
                self.notify_member(&task.to).await;
            }
        }

        Ok(())
    }

    /// 成员从发布区取任务
    pub async fn take(&self, member_id: &str) -> Option<DelegationTask> {
        let mut publish = self.publish.write().await;
        let pos = publish.iter().position(|t| t.to == member_id)?;
        Some(publish.remove(pos))
    }

    /// 取消任务（从发布区移除）
    pub async fn cancel(&self, task_id: &str) -> anyhow::Result<DelegationTask> {
        let mut publish = self.publish.write().await;
        if let Some(pos) = publish.iter().position(|t| t.id == task_id) {
            return Ok(publish.remove(pos));
        }
        anyhow::bail!("任务 '{}' 不存在", task_id)
    }

    /// 获取 Board 状态
    pub async fn status(&self) -> BoardStatus {
        let publish = self.publish.read().await;
        let chain = self.chain.read().await;
        BoardStatus {
            publish_count: publish.len(),
            head_pos: chain.head_pos(),
            current_task: chain.current_task().cloned(),
        }
    }

    /// 获取任务链快照
    pub async fn chain_snapshot(&self) -> TaskChain {
        self.chain.read().await.clone()
    }

    /// 成员阻塞等待直到有任务
    pub async fn wait(&self, member_id: &str) {
        let notify = {
            let mut map = self.notifies.write().await;
            map.entry(member_id.to_string())
                .or_insert_with(|| Arc::new(Notify::new()))
                .clone()
        };
        notify.notified().await;
    }

    async fn notify_member(&self, member_id: &str) {
        if let Some(notify) = self.notifies.read().await.get(member_id) {
            notify.notify_one();
        }
    }

    // ── Schedule 委派 ──────────────────────────────────────────

    pub fn schedule_add(&self, target: &str, description: &str, priority: i32) -> anyhow::Result<i64> {
        self.db.schedule_add(target, description, priority)
    }

    pub fn schedule_list(&self) -> Vec<crate::memory::ScheduleEntry> {
        self.db.schedule_list()
    }

    pub fn schedule_pop(&self, target: &str) -> Option<crate::memory::ScheduleEntry> {
        self.db.schedule_pop(target)
    }

    pub fn schedule_remove(&self, id: i64) -> anyhow::Result<()> {
        self.db.schedule_remove(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Arc<SqliteMemory> {
        let temp = std::env::temp_dir().join(format!("test_{}.db", uuid::Uuid::new_v4()));
        Arc::new(SqliteMemory::open(&temp.to_string_lossy()).unwrap())
    }

    #[tokio::test]
    async fn test_board_offer_and_take() {
        let db = test_db();
        let board = DelegationBoard::new(db, vec!["alice".to_string(), "bob".to_string()]);

        let task_id = board
            .offer("user", "alice", "测试任务", "详细描述", None)
            .await
            .unwrap();

        let task = board.take("alice").await.unwrap();
        assert_eq!(task.id, task_id);
        assert_eq!(task.to, "alice");
        assert_eq!(task.brief, "测试任务");
    }

    #[tokio::test]
    async fn test_board_result_done() {
        let db = test_db();
        let board = DelegationBoard::new(db, vec!["alice".to_string()]);

        let task_id = board
            .offer("user", "alice", "任务", "详情", None)
            .await
            .unwrap();

        board.take("alice").await.unwrap();

        board
            .result(
                "alice",
                TaskResult {
                    delegation_id: task_id,
                    detail: "完成".to_string(),
                    summary: "总结".to_string(),
                    done: true,
                    reply: String::new(),
                },
            )
            .await
            .unwrap();

        // done=true 后发布区应为空
        let status = board.status().await;
        assert_eq!(status.publish_count, 0);
    }

    #[tokio::test]
    async fn test_board_invalid_member() {
        let db = test_db();
        let board = DelegationBoard::new(db, vec!["alice".to_string()]);

        let result = board.offer("user", "bob", "任务", "详情", None).await;
        assert!(result.is_err());
    }
}
