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

use crate::memory::{
    DelegationRecord, MemoryEntry, PersonalMemory, SqliteMemory, Worklog, WorklogEntry,
};

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
    pub brief: String,
    pub detail: String,
    pub parent_id: Option<String>,
    pub priority: u32,
    pub result: Option<String>,
    pub reject_count: u32,
}

impl DelegationTask {
    fn new(
        from: &str,
        to: &str,
        brief: &str,
        detail: &str,
        parent_id: Option<&str>,
        priority: u32,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            from: from.to_string(),
            to: to.to_string(),
            brief: brief.to_string(),
            detail: detail.to_string(),
            parent_id: parent_id.map(|s| s.to_string()),
            priority,
            result: None,
            reject_count: 0,
        }
    }

    /// 格式化输出完整委托信息
    pub fn format(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("[委托 {}]\n", self.id));
        out.push_str(&format!("简述: {}\n", self.brief));
        if !self.detail.is_empty() {
            out.push_str(&format!("详情: {}\n", self.detail));
        }
        out.push_str(&format!("委托人: {}\n", self.from));
        out.push_str(&format!("承接人: {}\n", self.to));
        if let Some(ref pid) = self.parent_id {
            out.push_str(&format!("上级委托: {}\n", pid));
        }
        if let Some(ref result) = self.result {
            if !result.is_empty() {
                out.push_str(&format!("结果:\n{}\n", result));
            }
        }
        out
    }

    /// 克隆并重置为可重新发布的状态
    fn clone_for_reoffer(&self) -> Self {
        Self {
            id: self.id.clone(),
            from: self.from.clone(),
            to: self.to.clone(),
            brief: self.brief.clone(),
            detail: self.detail.clone(),
            parent_id: self.parent_id.clone(),
            priority: self.priority,
            result: None,
            reject_count: self.reject_count,
        }
    }
}

/// 委托板——四区任务队列
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
        brief: &str,
        detail: &str,
        parent_id: Option<&str>,
        priority: u32,
    ) -> Result<String> {
        let task = DelegationTask::new(from, to, brief, detail, parent_id, priority);
        let id = task.id.clone();

        // 持久化到 SQLite
        let _ = self
            .db
            .delegation_insert(&id, from, to, brief, detail, parent_id);

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

    /// 成员提交结果：从执行区移到返回区，写入个人记忆
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

        // 同步 result 到 SQLite
        let _ = self.db.delegation_set_result(task_id, result);

        let returned_task = task.clone();
        let mut returned = self.returned.write().await;
        returned.push(task);

        // 写入个人记忆（谁完成谁记录）
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let _ = self.db.remember(MemoryEntry {
            timestamp: now,
            delegation_id: returned_task.id.clone(),
            from_agent: returned_task.from.clone(),
            description: returned_task.brief.clone(),
            summary: summary.to_string(),
            artifacts: vec![],
            tags: vec![],
        });
        Ok(())
    }

    /// 发布者接受结果：从返回区删除，写入工作日志
    ///
    /// 谁发布谁负责记工作日志。个人记忆在 return_task 时已写。
    pub async fn accept(&self, member_id: &str, task_id: &str) -> Result<DelegationTask> {
        let mut returned = self.returned.write().await;
        let pos = returned
            .iter()
            .position(|t| t.id == task_id && t.from == member_id)
            .ok_or_else(|| anyhow::anyhow!("返回区中未找到任务 '{}'", task_id))?;

        let task = returned.remove(pos);

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let _ = self.db.append(WorklogEntry {
            timestamp: now,
            agent_id: task.to.clone(),
            delegation_id: task.id.clone(),
            to_agent: task.to.clone(),
            description: task.brief.clone(),
            summary: task.result.clone().unwrap_or_default(),
            artifacts: vec![],
        });

        Ok(task)
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
            publish_details: publish.iter().map(TaskInfo::from).collect(),
            exec_details: exec.iter().map(TaskInfo::from).collect(),
            returned_details: returned.iter().map(TaskInfo::from).collect(),
        }
    }

    /// 按 ID 查询委托记录（用于链向上查找）
    pub fn get_task(&self, id: &str) -> Option<DelegationRecord> {
        self.db.delegation_get(id)
    }

    /// 向委托 result 字段追加子委托的完整输出
    pub fn append_result(&self, task_id: &str, text: &str) -> Result<()> {
        self.db.delegation_append_result(task_id, text)
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

    pub fn schedule_revert(&self, id: i64) {
        self.db.schedule_revert(id)
    }

    pub fn schedule_remove(&self, id: i64) -> Result<()> {
        self.db.schedule_remove(id)
    }

    pub fn schedule_reorder(&self, id: i64, new_priority: i32) -> Result<()> {
        self.db.schedule_reorder(id, new_priority)
    }

    /// 回忆：按关键词搜索 worklog + personal_memory
    pub fn recall(&self, keyword: &str) -> String {
        let worklog_hits = self.db.search(keyword);
        let memory_hits = self.db.recall(keyword);

        let mut lines = Vec::new();
        if !worklog_hits.is_empty() {
            lines.push("工作日志匹配：".to_string());
            for e in &worklog_hits {
                lines.push(format!(
                    "  [{}] {}: {}",
                    e.delegation_id, e.agent_id, e.summary
                ));
            }
        }
        if !memory_hits.is_empty() {
            if !lines.is_empty() {
                lines.push(String::new());
            }
            lines.push("个人记忆匹配：".to_string());
            for m in &memory_hits {
                lines.push(format!(
                    "  [{}] {} — tags: {}",
                    m.delegation_id,
                    m.summary.chars().take(200).collect::<String>(),
                    m.tags.join(", ")
                ));
            }
        }
        if lines.is_empty() {
            format!("未找到与 '{}' 相关的记忆", keyword)
        } else {
            lines.join("\n")
        }
    }
}

/// 委托板快照
#[derive(Debug, Clone, Serialize)]
pub struct BoardStatus {
    pub publish_count: usize,
    pub exec_count: usize,
    pub returned_count: usize,
    pub publish_details: Vec<TaskInfo>,
    pub exec_details: Vec<TaskInfo>,
    pub returned_details: Vec<TaskInfo>,
}

/// 任务简要信息
#[derive(Debug, Clone, Serialize)]
pub struct TaskInfo {
    pub id: String,
    pub from: String,
    pub to: String,
    pub brief: String,
}

impl From<&DelegationTask> for TaskInfo {
    fn from(t: &DelegationTask) -> Self {
        Self {
            id: t.id.clone(),
            from: t.from.clone(),
            to: t.to.clone(),
            brief: t.brief.clone(),
        }
    }
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
            .offer("architect", "coder", "写代码", "实现用户认证模块", None, 0)
            .await
            .unwrap();
        assert!(!task_id.is_empty());
        assert_eq!(board.status().await.publish_count, 1);

        // claim
        let task = board.claim("coder").await.unwrap().unwrap();
        assert_eq!(task.id, task_id);
        assert_eq!(task.brief, "写代码");
        assert_eq!(board.status().await.publish_count, 0);
        assert_eq!(board.status().await.exec_count, 1);

        // return
        board
            .return_task("coder", &task_id, "fn main() {}", "实现了入口")
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
            .offer("architect", "coder", "写代码", "实现认证", None, 0)
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

        let retry = board.claim("coder").await.unwrap().unwrap();
        assert_eq!(retry.id, task_id);
        assert_eq!(retry.reject_count, 1);
    }

    #[tokio::test]
    async fn reject_over_threshold_escalates_to_manager() {
        let board = DelegationBoard::new(test_db());

        let task_id = board
            .offer("architect", "coder", "写代码", "实现认证", None, 0)
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

        let task = board.claim("manager").await.unwrap().unwrap();
        assert_eq!(task.id, task_id);
        assert_eq!(task.to, "manager");
        assert_eq!(task.reject_count, DISPUTE_THRESHOLD);
    }

    #[tokio::test]
    async fn user_message_priority() {
        let board = DelegationBoard::new(test_db());
        board
            .offer("architect", "coder", "普通任务", "", None, 0)
            .await
            .unwrap();
        board
            .offer("user", "coder", "紧急消息", "", None, PRIORITY_USER)
            .await
            .unwrap();

        let task = board.claim("coder").await.unwrap().unwrap();
        assert_eq!(task.brief, "紧急消息");
    }

    #[tokio::test]
    async fn check_return_lists_pending_review() {
        let board = DelegationBoard::new(test_db());
        let task_id = board
            .offer("architect", "coder", "写代码", "", None, 0)
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

        let empty = board.check_return("coder").await;
        assert!(empty.is_empty());
    }

    #[tokio::test]
    async fn cancel_from_exec() {
        let board = DelegationBoard::new(test_db());
        let task_id = board
            .offer("architect", "coder", "写代码", "", None, 0)
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
            .offer("architect", "coder", "不需要了", "", None, 0)
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
    async fn is_working_tracks_exec_zone() {
        let board = DelegationBoard::new(test_db());
        assert!(!board.is_working("coder").await);

        board
            .offer("architect", "coder", "写代码", "", None, 0)
            .await
            .unwrap();
        board.claim("coder").await.unwrap();
        assert!(board.is_working("coder").await);

        assert!(!board.is_working("designer").await);
    }

    #[tokio::test]
    async fn delegation_persisted_to_sqlite() {
        let board = DelegationBoard::new(test_db());
        let task_id = board
            .offer("architect", "coder", "写API", "实现 GET /users", None, 0)
            .await
            .unwrap();

        let record = board.get_task(&task_id).unwrap();
        assert_eq!(record.brief, "写API");
        assert_eq!(record.detail, "实现 GET /users");
        assert_eq!(record.from_agent, "architect");
        assert_eq!(record.to_agent, "coder");
    }

    #[tokio::test]
    async fn delegation_format_output() {
        let board = DelegationBoard::new(test_db());
        let _task_id = board
            .offer("manager", "coder", "实现JWT", "签发和验证令牌", None, 0)
            .await
            .unwrap();

        let task = board.claim("coder").await.unwrap().unwrap();
        let formatted = task.format();
        assert!(formatted.contains("[委托"));
        assert!(formatted.contains("简述: 实现JWT"));
        assert!(formatted.contains("详情: 签发和验证令牌"));
        assert!(formatted.contains("委托人: manager"));
        assert!(formatted.contains("承接人: coder"));
    }

    #[tokio::test]
    async fn append_result_downward() {
        let board = DelegationBoard::new(test_db());
        let parent_id = board
            .offer("manager", "coder", "父任务", "", None, 0)
            .await
            .unwrap();

        // 模拟子委托完成，追加到父的 result
        board
            .append_result(
                &parent_id,
                "\n---\n[委托 child-1]\n简述: 子任务\n结果: 完成\n",
            )
            .unwrap();

        let record = board.get_task(&parent_id).unwrap();
        assert!(record.result.contains("子任务"));
        assert!(record.result.contains("完成"));
    }

    #[tokio::test]
    async fn parent_id_chain_lookup() {
        let board = DelegationBoard::new(test_db());
        let root_id = board
            .offer("user", "manager", "构建系统", "", None, PRIORITY_USER)
            .await
            .unwrap();

        let child_id = board
            .offer("manager", "coder", "实现JWT", "", Some(&root_id), 0)
            .await
            .unwrap();

        // 通过 parent_id 向上查
        let child = board.get_task(&child_id).unwrap();
        assert_eq!(child.parent_id.as_deref(), Some(root_id.as_str()));

        let root = board.get_task(&root_id).unwrap();
        assert_eq!(root.brief, "构建系统");
    }

    #[tokio::test]
    async fn return_task_updates_sqlite_result() {
        let board = DelegationBoard::new(test_db());
        let task_id = board
            .offer("architect", "coder", "写代码", "", None, 0)
            .await
            .unwrap();

        board.claim("coder").await.unwrap();
        board
            .return_task("coder", &task_id, "完成: src/main.rs", "ok")
            .await
            .unwrap();

        let record = board.get_task(&task_id).unwrap();
        assert_eq!(record.result, "完成: src/main.rs");
    }
}
