//! 委托板（Delegation Board）+ 任务链（TaskChain）
//!
//! 简化后的双区模型：发布区 + 档案区（SQLite）。
//! 任务链以图灵机方式管理委托过程：链右移（delegate），链左移（finish，done=true）。
//!
//! ```text
//! offer ──→ [发布区] ──take──→ [成员处理] ──finish/delegate──→ [档案区: SQLite]
//! ```

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Notify, RwLock, broadcast};
use types::message::{ChatMessage, MessageContent, Role};

use crate::memory::{PersonalMemory, SqliteMemory, Worklog};

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
    pub(crate) entries: Vec<ChainEntry>,
    pub(crate) head: usize,
}

impl TaskChain {
    // TODO: 虚拟根条目会污染 assemble_prompt 的链上下文输出，后续考虑用 Option<TaskChain> 消除
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

    /// 当前链头上的委托是否已有产出（用在 execute_task 里区分初执/续执）
    pub fn has_outcome(&self) -> bool {
        self.entries.iter().any(
            |e| matches!(e, ChainEntry::Outcome { delegate_idx, .. } if *delegate_idx == self.head),
        )
    }

    /// 根据 ChainEntry 类型自动写入 worklog：
    /// Delegate → insert 新行，Outcome → update summary
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

// ── Board ───────────────────────────────────────────────

/// 委托板——发布区 + 档案区（SQLite）+ 任务链
pub struct DelegationBoard {
    publish: RwLock<Vec<DelegationTask>>,
    db: Arc<SqliteMemory>,
    chain: RwLock<TaskChain>,
    cached_worklog: RwLock<String>,
    system_prompt: String,
    member_roster: String,
    events: RwLock<HashMap<String, broadcast::Sender<String>>>,
    notifies: RwLock<HashMap<String, Arc<Notify>>>,
}

// ── Lifecycle ────────────────────────────────────────────────

impl DelegationBoard {
    pub fn new(db: Arc<SqliteMemory>, system_prompt: String, member_roster: String) -> Self {
        let init_worklog = Self::format_worklog(&db);
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
            system_prompt,
            member_roster,
            cached_worklog: RwLock::new(init_worklog),
            events: {
                let mut map = HashMap::new();
                let (tx, _) = broadcast::channel(32);
                map.insert(types::event_index::CH_RAW_EVENTS.into(), tx);
                let (tx, _) = broadcast::channel(32);
                map.insert(types::event_index::CH_WORKSPACE_STREAM.into(), tx);
                RwLock::new(map)
            },
            notifies: RwLock::new(HashMap::new()),
        }
    }

    pub fn db(&self) -> &SqliteMemory {
        &self.db
    }

    /// 为 delegation 注册一个 SSE 事件监听器
    pub async fn register_listener(
        &self,
        delegation_id: &str,
    ) -> Option<broadcast::Receiver<String>> {
        self.events
            .read()
            .await
            .get(delegation_id)
            .map(|tx| tx.subscribe())
    }

    /// 成员阻塞等待直到链头指向自己
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

    /// 清理 delegation 的事件通道
    pub async fn cleanup_events(&self, delegation_id: &str) {
        let mut map = self.events.write().await;
        map.remove(delegation_id);
    }

    /// 获取任务链快照（只读克隆，供 Prompt 使用）
    pub async fn chain_snapshot(&self) -> TaskChain {
        self.chain.read().await.clone()
    }

    /// 通知 server 层有事件发生（server 负责从 state 构造完整消息并推送）
    /// payload 格式: "event_name\noptional_data"
    async fn trigger_event(&self, event_name: &str, data: &str) {
        let payload = if data.is_empty() {
            event_name.to_string()
        } else {
            format!("{}\n{}", event_name, data)
        };
        self.publish_raw(types::event_index::CH_RAW_EVENTS, &payload).await;
    }

    /// 推送一条原始数据到指定 delegation
    pub async fn publish_raw(&self, delegation_id: &str, data: &str) {
        let map = self.events.read().await;
        if let Some(tx) = map.get(delegation_id) {
            let _ = tx.send(data.to_string());
        }
    }

    /// 组装 LLM 上下文消息（替代原 Prompt::format）
    pub async fn assemble_prompt(&self, soul: &str, memories: &str) -> Vec<ChatMessage> {
        let mut messages = Vec::new();

        // 1. 系统提示词（最前面）
        if !self.system_prompt.is_empty() {
            messages.push(ChatMessage {
                role: Role::User,
                content: MessageContent::Text(self.system_prompt.clone()),
                name: None,
                tool_calls: None,
                reasoning_content: None,
            });
        }

        // 2. 团队成员名录
        if !self.member_roster.is_empty() {
            messages.push(ChatMessage {
                role: Role::User,
                content: MessageContent::Text(format!("--- 团队成员 ---\n{}", self.member_roster,)),
                name: None,
                tool_calls: None,
                reasoning_content: None,
            });
        }

        // 3. 团队工作日志（内存缓存，根委托完成时刷新）
        {
            let worklog = self.cached_worklog.read().await;
            if !worklog.is_empty() {
                messages.push(ChatMessage {
                    role: Role::User,
                    content: MessageContent::Text(format!("--- 团队工作日志 ---\n{}", *worklog)),
                    name: None,
                    tool_calls: None,
                    reasoning_content: None,
                });
            }
        }

        // 4. 委托链上下文
        let chain = self.chain.read().await;
        let chain_text = Self::format_chain(&chain);
        messages.push(ChatMessage {
            role: Role::User,
            content: MessageContent::Text(format!("--- 委托链上下文 ---\n{}", chain_text)),
            name: None,
            tool_calls: None,
            reasoning_content: None,
        });

        // 5. 人格提示词（system 消息，工作日志后面、个人记忆前面）
        messages.push(ChatMessage {
            role: Role::System,
            content: MessageContent::Text(soul.to_string()),
            name: None,
            tool_calls: None,
            reasoning_content: None,
        });

        // 6. 个人记忆
        if !memories.is_empty() {
            messages.push(ChatMessage {
                role: Role::User,
                content: MessageContent::Text(format!("--- 你的相关记忆 ---\n{}", memories)),
                name: None,
                tool_calls: None,
                reasoning_content: None,
            });
        }

        // 7. 当前任务（永远最后）
        if let Some(task) = chain.current_task() {
            let mut task_desc = format!("当前任务: {}", task.brief);
            if !task.detail.is_empty() {
                task_desc.push_str(&format!("\n详情: {}", task.detail));
            }
            messages.push(ChatMessage {
                role: Role::User,
                content: MessageContent::Text(task_desc),
                name: None,
                tool_calls: None,
                reasoning_content: None,
            });
        }

        messages
    }

    fn format_chain(chain: &TaskChain) -> String {
        let mut lines = Vec::new();
        for (i, entry) in chain.entries.iter().enumerate() {
            let marker = if i == chain.head { "→ " } else { "  " };
            match entry {
                ChainEntry::Delegate { task, .. } => {
                    let short_id = if task.id.is_empty() {
                        "(root)"
                    } else {
                        &task.id[..task.id.len().min(8)]
                    };
                    lines.push(format!(
                        "{}[委托 {}] {} 委托 {}: {}",
                        marker, short_id, task.from, task.to, task.brief
                    ));
                    if !task.detail.is_empty() {
                        lines.push(format!("  详情: {}", task.detail));
                    }
                    if let Some(ref pid) = task.parent_id {
                        let short_pid = &pid[..pid.len().min(8)];
                        lines.push(format!("  上级: {}", short_pid));
                    }
                }
                ChainEntry::Outcome {
                    delegate_idx,
                    result,
                } => {
                    let status = if result.done {
                        "完成"
                    } else {
                        "阶段性产出"
                    };
                    lines.push(format!(
                        "{}[产出 idx={}] {} — {}",
                        marker, delegate_idx, result.summary, status
                    ));
                }
            }
        }
        lines.join("\n")
    }

    fn format_worklog(db: &SqliteMemory) -> String {
        let entries = db.recent(20);
        if entries.is_empty() {
            return String::new();
        }
        entries
            .iter()
            .map(|e| format!("[{}] {} — {}", e.agent_id, e.delegation_id, e.summary))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

// ── Operations ───────────────────────────────────────────────

impl DelegationBoard {
    /// 发布委托到发布区，同时链右移
    pub async fn offer(
        &self,
        from: &str,
        to: &str,
        brief: &str,
        detail: &str,
        parent_id: Option<&str>,
    ) -> Result<String> {
        let task = DelegationTask::new(from, to, brief, detail, parent_id);
        let id = task.id.clone();

        // 预建 SSE 事件通道
        {
            let mut map = self.events.write().await;
            map.entry(id.clone()).or_insert_with(|| {
                let (tx, _) = broadcast::channel(32);
                tx
            });
        }

        self.db
            .delegation_insert(&id, from, to, brief, detail, parent_id)?;

        // 先入链（worklog/memory 由 add_task 内部写入）
        let cloned = {
            let mut chain = self.chain.write().await;
            chain.add_task(task, &self.db);
            chain.entries.last().and_then(|e| match e {
                ChainEntry::Delegate { task, .. } => Some(task.clone()),
                _ => None,
            })
        };

        // 放掉链锁后再入发布区
        if let Some(t) = cloned {
            let target = t.to.clone();
            let from = t.from.clone();
            let mut publish = self.publish.write().await;
            publish.push(t);
            let cur = publish.len();
            drop(publish);
            tracing::info!("board: publish={} | from={} → to={}", cur, from, target);
            self.trigger_event(types::event_index::EVT_BOARD_UPDATE, &cur.to_string()).await;
            self.notify_member(&target).await;
        }

        Ok(id)
    }

    /*
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
    */

    /// 查询发布区中匹配成员的委托（不修改发布区）
    pub async fn take(&self, member_id: &str) -> Option<DelegationTask> {
        let publish = self.publish.read().await;
        publish.iter().find(|t| t.to == member_id).cloned()
    }

    /// 产出结果：链追加 Outcome；done 时从发布区移除
    pub async fn result(&self, member_id: &str, result: TaskResult) -> Result<()> {
        let task_id = result.delegation_id.clone();
        let done = result.done;

        // 链操作（含 worklog/memory 写入）
        {
            let mut chain = self.chain.write().await;
            chain.add_result(result, &self.db);
        }

        if !done {
            return Ok(());
        }

        // 从发布区移除（可能已被 take() 取走，忽略）
        let mut tasks = self.publish.write().await;
        if let Some(pos) = tasks
            .iter()
            .position(|t| t.id == task_id && t.to == member_id)
        {
            tasks.remove(pos);
            let cur = tasks.len();
            tracing::info!("board: publish={} | archived by {}", cur, member_id);
            drop(tasks);
            self.trigger_event(types::event_index::EVT_BOARD_UPDATE, &cur.to_string()).await;
        }

        // 唤醒上级（链头已左移，新链头指向父委托的承接者）
        if let Some(task) = self.chain.read().await.current_task() {
            if !task.id.is_empty() {
                self.notify_member(&task.to).await;
            } else {
                // 根委托完成，刷新工作日志缓存
                *self.cached_worklog.write().await = Self::format_worklog(&self.db);
            }
        }

        Ok(())
    }

    /// 取消任务
    pub async fn cancel(&self, task_id: &str) -> Result<DelegationTask> {
        let mut tasks = self.publish.write().await;
        if let Some(pos) = tasks.iter().position(|t| t.id == task_id) {
            return Ok(tasks.remove(pos));
        }
        bail!("任务 '{}' 不存在", task_id)
    }

    /// 查询快照
    pub async fn status(&self) -> BoardStatus {
        let tasks = self.publish.read().await;
        BoardStatus {
            publish_count: tasks.len(),
            publish_details: tasks.iter().map(TaskInfo::from).collect(),
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
    pub publish_details: Vec<TaskInfo>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::SqliteMemory;

    fn test_board() -> DelegationBoard {
        let db = Arc::new(SqliteMemory::open(":memory:").unwrap());
        DelegationBoard::new(db, String::new(), String::new())
    }

    #[tokio::test]
    async fn test_chain_offer_and_finish() {
        let board = test_board();

        let id = board
            .offer("user", "manager", "test", "", None)
            .await
            .unwrap();
        assert_eq!(board.chain.read().await.head, 1);

        board
            .result(
                "manager",
                TaskResult {
                    delegation_id: id,
                    detail: "done".into(),
                    summary: "done".into(),
                    done: true,
                    reply: String::new(),
                },
            )
            .await
            .unwrap();

        let chain = board.chain.read().await;
        assert_eq!(chain.head, 0);
        assert!(chain.current_task().unwrap().id.is_empty());
    }

    #[tokio::test]
    async fn test_chain_delegate_then_finish() {
        let board = test_board();

        let id1 = board
            .offer("user", "manager", "分发", "", None)
            .await
            .unwrap();

        board
            .result(
                "manager",
                TaskResult {
                    delegation_id: id1.clone(),
                    detail: "分发中".into(),
                    summary: "分发".into(),
                    done: false,
                    reply: String::new(),
                },
            )
            .await
            .unwrap();

        let id2 = board
            .offer("manager", "coder", "子任务", "直接finish", Some(&id1))
            .await
            .unwrap();

        board
            .result(
                "coder",
                TaskResult {
                    delegation_id: id2,
                    detail: "coder done".into(),
                    summary: "coder done".into(),
                    done: true,
                    reply: String::new(),
                },
            )
            .await
            .unwrap();

        // 链头回到 manager 的委托
        assert_eq!(board.chain.read().await.head, 1);

        // manager finish 回到根
        board
            .result(
                "manager",
                TaskResult {
                    delegation_id: id1,
                    detail: "manager done".into(),
                    summary: "manager done".into(),
                    done: true,
                    reply: String::new(),
                },
            )
            .await
            .unwrap();
        assert_eq!(board.chain.read().await.head, 0);
    }

    #[tokio::test]
    async fn test_chain_multi_level_delegate() {
        let board = test_board();

        // user → manager
        let id1 = board
            .offer("user", "manager", "顶层任务", "", None)
            .await
            .unwrap();

        // manager → coder
        board
            .result(
                "manager",
                TaskResult {
                    delegation_id: id1.clone(),
                    detail: "分派中".into(),
                    summary: "分派".into(),
                    done: false,
                    reply: String::new(),
                },
            )
            .await
            .unwrap();
        let id2 = board
            .offer("manager", "coder", "子任务", "", Some(&id1))
            .await
            .unwrap();

        // coder → reviewer
        board
            .result(
                "coder",
                TaskResult {
                    delegation_id: id2.clone(),
                    detail: "再次分派".into(),
                    summary: "再次分派".into(),
                    done: false,
                    reply: String::new(),
                },
            )
            .await
            .unwrap();
        let id3 = board
            .offer("coder", "reviewer", "孙任务", "", Some(&id2))
            .await
            .unwrap();

        // reviewer finish → 回到 coder
        board
            .result(
                "reviewer",
                TaskResult {
                    delegation_id: id3,
                    detail: "审查完成".into(),
                    summary: "审查完成".into(),
                    done: true,
                    reply: String::new(),
                },
            )
            .await
            .unwrap();
        assert_eq!(board.chain.read().await.head, 3); // 回到 coder 的委托

        // coder finish → 回到 manager
        board
            .result(
                "coder",
                TaskResult {
                    delegation_id: id2,
                    detail: "编码完成".into(),
                    summary: "编码完成".into(),
                    done: true,
                    reply: String::new(),
                },
            )
            .await
            .unwrap();
        assert_eq!(board.chain.read().await.head, 1); // 回到 manager 的委托

        // manager finish → 回到根
        board
            .result(
                "manager",
                TaskResult {
                    delegation_id: id1,
                    detail: "全部完成".into(),
                    summary: "全部完成".into(),
                    done: true,
                    reply: String::new(),
                },
            )
            .await
            .unwrap();
        assert_eq!(board.chain.read().await.head, 0); // 回到根
        assert!(
            board
                .chain
                .read()
                .await
                .current_task()
                .unwrap()
                .id
                .is_empty()
        );
    }

    #[tokio::test]
    async fn test_worklog_cache_refresh_on_root_done() {
        let board = test_board();

        // 初始缓存为空
        assert!(board.cached_worklog.read().await.is_empty());

        let id = board
            .offer("user", "manager", "测试任务", "", None)
            .await
            .unwrap();

        // 中间委托不刷新缓存
        board
            .result(
                "manager",
                TaskResult {
                    delegation_id: id.clone(),
                    detail: "中间产出".into(),
                    summary: "中间".into(),
                    done: false,
                    reply: String::new(),
                },
            )
            .await
            .unwrap();
        assert!(board.cached_worklog.read().await.is_empty());

        // 根完成 → 缓存刷新
        board
            .result(
                "manager",
                TaskResult {
                    delegation_id: id,
                    detail: "完成".into(),
                    summary: "完成".into(),
                    done: true,
                    reply: String::new(),
                },
            )
            .await
            .unwrap();
        assert!(!board.cached_worklog.read().await.is_empty());
    }
}
