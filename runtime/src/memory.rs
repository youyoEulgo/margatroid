//! Margatroid 记忆系统
//!
//! 两层架构：工作日志（团队共享）+ 个人记忆（每 agent 私有）。
//! delegation_id 是两表之间的 join key——Workspace 通过它做确定性检索。
//! SQLite 单文件后端，WAL 模式。

use anyhow::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

// ── 类型 ──────────────────────────────────────────────────────

/// 工作日志条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorklogEntry {
    pub timestamp: u64,
    pub agent_id: String,
    pub delegation_id: String,
    /// 委托目标
    pub to_agent: String,
    /// 委托描述
    pub description: String,
    /// 工作摘要（~30-50 token）
    pub summary: String,
    /// LLM 对话回复
    #[serde(default)]
    pub reply: String,
    #[serde(default)]
    pub artifacts: Vec<String>,
}

/// 个人记忆条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub timestamp: u64,
    pub delegation_id: String,
    /// 委托来源
    pub from_agent: String,
    pub description: String,
    pub summary: String,
    #[serde(default)]
    pub artifacts: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// 对话消息（LLM 回复的文本内容，每个回复一条）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationMessage {
    pub delegation_id: String,
    pub role: String,
    pub content: String,
    pub created_at: u64,
}

// ── Traits ────────────────────────────────────────────────────

/// 工作日志（团队共享）
pub trait Worklog: Send + Sync {
    fn recent(&self, limit: usize) -> Vec<WorklogEntry>;
    fn search(&self, keyword: &str) -> Vec<WorklogEntry>;
}

/// 个人记忆（每 agent 私有）
pub trait PersonalMemory: Send + Sync {
    fn recall(&self, keyword: &str) -> Vec<MemoryEntry>;
    fn recall_by_tag(&self, tag: &str) -> Vec<MemoryEntry>;
}

/// SQLite 记忆存储
///
/// 使用 Mutex 保证线程安全。workspace 内所有成员共享同一个连接。
pub struct SqliteMemory {
    conn: Mutex<Connection>,
}

/// 计划表条目（Manager 专用）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleEntry {
    pub id: i64,
    pub target: String,
    pub description: String,
    pub priority: i32,
    pub status: String, // planned | offered | archived
}

/// 委托持久化记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelegationRecord {
    pub id: String,
    pub from_agent: String,
    pub to_agent: String,
    pub brief: String,
    pub detail: String,
    pub parent_id: Option<String>,
    pub result: String,
}

impl DelegationRecord {
    /// 格式化输出完整委托信息（与 DelegationTask::format() 一致）
    pub fn format(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("[委托 {}]\n", self.id));
        out.push_str(&format!("简述: {}\n", self.brief));
        if !self.detail.is_empty() {
            out.push_str(&format!("详情: {}\n", self.detail));
        }
        out.push_str(&format!("委托人: {}\n", self.from_agent));
        out.push_str(&format!("承接人: {}\n", self.to_agent));
        if let Some(ref pid) = self.parent_id {
            out.push_str(&format!("上级委托: {}\n", pid));
        }
        if !self.result.is_empty() {
            out.push_str(&format!("结果:\n{}\n", self.result));
        }
        out
    }
}

impl SqliteMemory {
    /// 打开或创建数据库，初始化表
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;

        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS worklog (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp INTEGER NOT NULL,
                agent_id TEXT NOT NULL,
                delegation_id TEXT NOT NULL UNIQUE,
                to_agent TEXT NOT NULL DEFAULT '',
                description TEXT NOT NULL DEFAULT '',
                summary TEXT NOT NULL DEFAULT '',
                reply TEXT NOT NULL DEFAULT '',
                artifacts TEXT NOT NULL DEFAULT '[]'
            );
            CREATE INDEX IF NOT EXISTS idx_worklog_agent ON worklog(agent_id);
            CREATE INDEX IF NOT EXISTS idx_worklog_delegation ON worklog(delegation_id);

            CREATE TABLE IF NOT EXISTS personal_memory (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp INTEGER NOT NULL,
                agent_id TEXT NOT NULL,
                from_agent TEXT NOT NULL DEFAULT '',
                delegation_id TEXT NOT NULL UNIQUE,
                description TEXT NOT NULL DEFAULT '',
                summary TEXT NOT NULL DEFAULT '',
                artifacts TEXT NOT NULL DEFAULT '[]',
                tags TEXT NOT NULL DEFAULT '[]'
            );
            CREATE INDEX IF NOT EXISTS idx_personal_agent ON personal_memory(agent_id);
            CREATE INDEX IF NOT EXISTS idx_personal_delegation ON personal_memory(delegation_id);

            CREATE TABLE IF NOT EXISTS schedule (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                target TEXT NOT NULL,
                description TEXT NOT NULL,
                priority INTEGER NOT NULL DEFAULT 0,
                status TEXT NOT NULL DEFAULT 'planned'
            );
            CREATE INDEX IF NOT EXISTS idx_schedule_status ON schedule(status);

            CREATE TABLE IF NOT EXISTS delegations (
                id TEXT PRIMARY KEY,
                from_agent TEXT NOT NULL,
                to_agent TEXT NOT NULL,
                brief TEXT NOT NULL,
                detail TEXT NOT NULL DEFAULT '',
                parent_id TEXT,
                result TEXT NOT NULL DEFAULT ''
            );
            CREATE INDEX IF NOT EXISTS idx_delegations_parent ON delegations(parent_id);

            CREATE TABLE IF NOT EXISTS conversation_messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                delegation_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                created_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_conv_delegation ON conversation_messages(delegation_id);
            ",
        )?;

        // migration: add reply column for existing databases
        let _ = conn.execute_batch("ALTER TABLE worklog ADD COLUMN reply TEXT NOT NULL DEFAULT '';");

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    // ── 供 Workspace 使用的扩展查询 ──

    /// 查询某个 agent 的近期工作日志条目（附带 delegation_id）
    pub fn worklog_by_agent(&self, agent_id: &str, limit: usize) -> Result<Vec<WorklogEntry>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT timestamp, agent_id, delegation_id, to_agent, description, summary, reply, artifacts
             FROM worklog
             WHERE agent_id = ?
             ORDER BY timestamp DESC
             LIMIT ?",
        )?;
        let entries = stmt
            .query_map(rusqlite::params![agent_id, limit as i64], row_to_worklog)?
            .filter_map(|r| r.ok())
            .collect();
        Ok(entries)
    }

    /// 通过 delegation_id 直接查 personal memory
    pub fn personal_by_delegation(&self, delegation_id: &str) -> Option<MemoryEntry> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT timestamp, agent_id, from_agent, delegation_id, description, summary, artifacts, tags
             FROM personal_memory
             WHERE delegation_id = ?",
            rusqlite::params![delegation_id],
            row_to_memory,
        )
        .ok()
    }

    /// 通过 delegation_id 列表批量查 personal memory
    pub fn personal_by_delegations(&self, ids: &[String]) -> Vec<MemoryEntry> {
        if ids.is_empty() {
            return vec![];
        }
        let conn = self.conn.lock().unwrap();
        let placeholders: Vec<String> = ids
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", i + 1))
            .collect();
        let sql = format!(
            "SELECT timestamp, agent_id, from_agent, delegation_id, description, summary, artifacts, tags
             FROM personal_memory
             WHERE delegation_id IN ({})",
            placeholders.join(", ")
        );
        let params: Vec<&dyn rusqlite::types::ToSql> = ids
            .iter()
            .map(|id| id as &dyn rusqlite::types::ToSql)
            .collect();

        match conn.prepare(&sql) {
            Ok(mut stmt) => stmt
                .query_map(params.as_slice(), row_to_memory)
                .ok()
                .map(|rows| rows.filter_map(|r| r.ok()).collect())
                .unwrap_or_default(),
            Err(_) => vec![],
        }
    }

    /// 统计条目数
    pub fn count_worklog(&self) -> usize {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT COUNT(*) FROM worklog", [], |r| r.get::<_, i64>(0))
            .map(|c| c as usize)
            .unwrap_or(0)
    }

    // ── 计划表操作 ──

    /// 添加计划条目
    pub fn schedule_add(&self, target: &str, description: &str, priority: i32) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO schedule (target, description, priority) VALUES (?, ?, ?)",
            rusqlite::params![target, description, priority],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// 列出所有 planned 状态的条目（按优先级排序）
    pub fn schedule_list(&self) -> Vec<ScheduleEntry> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT id, target, description, priority, status FROM schedule WHERE status = 'planned' ORDER BY priority DESC",
        ) {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        stmt.query_map([], |r| {
            Ok(ScheduleEntry {
                id: r.get(0)?,
                target: r.get(1)?,
                description: r.get(2)?,
                priority: r.get(3)?,
                status: r.get(4)?,
            })
        })
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    /// 取出指定成员的下一个任务（最高优先级），标记为 offered
    pub fn schedule_pop(&self, target: &str) -> Option<ScheduleEntry> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, target, description, priority, status FROM schedule
                 WHERE status = 'planned' AND target = ?
                 ORDER BY priority DESC LIMIT 1",
            )
            .ok()?;
        let entry = stmt
            .query_row(rusqlite::params![target], |r| {
                Ok(ScheduleEntry {
                    id: r.get(0)?,
                    target: r.get(1)?,
                    description: r.get(2)?,
                    priority: r.get(3)?,
                    status: r.get(4)?,
                })
            })
            .ok()?;
        let _ = conn.execute(
            "UPDATE schedule SET status = 'offered' WHERE id = ?",
            rusqlite::params![entry.id],
        );
        Some(entry)
    }

    /// 将条目标记为 archived
    pub fn schedule_archive(&self, id: i64) {
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute(
            "UPDATE schedule SET status = 'archived' WHERE id = ?",
            rusqlite::params![id],
        );
    }

    /// 查询指定成员是否有 offered 状态的阶段任务
    pub fn has_offered_schedule(&self, target: &str) -> bool {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM schedule WHERE target = ? AND status = 'offered'",
            rusqlite::params![target],
            |r| r.get::<_, i64>(0),
        )
        .map(|c| c > 0)
        .unwrap_or(false)
    }

    /// 将已 pop 的条目回退为 planned
    pub fn schedule_revert(&self, id: i64) {
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute(
            "UPDATE schedule SET status = 'planned' WHERE id = ?",
            rusqlite::params![id],
        );
    }

    /// 删除计划条目
    pub fn schedule_remove(&self, id: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM schedule WHERE id = ?", rusqlite::params![id])?;
        Ok(())
    }

    /// 调整优先级
    pub fn schedule_reorder(&self, id: i64, new_priority: i32) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE schedule SET priority = ? WHERE id = ?",
            rusqlite::params![new_priority, id],
        )?;
        Ok(())
    }

    // ── 委托持久化 ──

    /// 创建委托时写入 delegations 表
    pub fn delegation_insert(
        &self,
        id: &str,
        from: &str,
        to: &str,
        brief: &str,
        detail: &str,
        parent_id: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO delegations (id, from_agent, to_agent, brief, detail, parent_id, result)
             VALUES (?, ?, ?, ?, ?, ?, '')",
            rusqlite::params![id, from, to, brief, detail, parent_id],
        )?;
        Ok(())
    }

    /// 按 ID 查询单条委托
    pub fn delegation_get(&self, id: &str) -> Option<DelegationRecord> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, from_agent, to_agent, brief, detail, parent_id, result
             FROM delegations WHERE id = ?",
            rusqlite::params![id],
            |r| {
                Ok(DelegationRecord {
                    id: r.get(0)?,
                    from_agent: r.get(1)?,
                    to_agent: r.get(2)?,
                    brief: r.get(3)?,
                    detail: r.get(4)?,
                    parent_id: r.get(5)?,
                    result: r.get(6)?,
                })
            },
        )
        .ok()
    }

    /// 向委托的 result 字段追加文本
    pub fn delegation_append_result(&self, id: &str, text: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE delegations SET result = result || ? WHERE id = ?",
            rusqlite::params![text, id],
        )?;
        Ok(())
    }

    /// 更新委托的 result 字段（覆盖）
    pub fn delegation_set_result(&self, id: &str, result: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE delegations SET result = ? WHERE id = ?",
            rusqlite::params![result, id],
        )?;
        Ok(())
    }

    // ── 工作日志写入 ──

    /// 任务创建时插入 worklog 行（summary 留空，待产出时补全）
    pub fn worklog_add_task(
        &self,
        delegation_id: &str,
        from_agent: &str,
        to_agent: &str,
        description: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        conn.execute(
            "INSERT INTO worklog (timestamp, agent_id, delegation_id, to_agent, description, summary, artifacts)
             VALUES (?, ?, ?, ?, ?, '', '[]')",
            rusqlite::params![now as i64, from_agent, delegation_id, to_agent, description],
        )?;
        Ok(())
    }

    /// 任务产出时补全 worklog 的 summary
    pub fn worklog_add_result(&self, delegation_id: &str, summary: &str, reply: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE worklog SET summary = ?, reply = ? WHERE delegation_id = ?",
            rusqlite::params![summary, reply, delegation_id],
        )?;
        Ok(())
    }

    // ── 对话消息 ──

    /// 保存一条 assistant 对话消息
    pub fn conversation_add(&self, delegation_id: &str, role: &str, content: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        conn.execute(
            "INSERT INTO conversation_messages (delegation_id, role, content, created_at)
             VALUES (?, ?, ?, ?)",
            rusqlite::params![delegation_id, role, content, now as i64],
        )?;
        Ok(())
    }

    /// 查询某个委托下的所有对话消息
    pub fn conversation_messages(&self, delegation_id: &str) -> Vec<ConversationMessage> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT delegation_id, role, content, created_at
             FROM conversation_messages
             WHERE delegation_id = ?
             ORDER BY created_at ASC",
        ) {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        stmt.query_map(rusqlite::params![delegation_id], |row| {
            Ok(ConversationMessage {
                delegation_id: row.get(0)?,
                role: row.get(1)?,
                content: row.get(2)?,
                created_at: row.get::<_, i64>(3)? as u64,
            })
        })
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    /// 返回所有活跃委托的最新对话消息（限制条数）
    pub fn recent_conversations(&self, limit: usize) -> Vec<ConversationMessage> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT delegation_id, role, content, created_at
             FROM conversation_messages
             ORDER BY created_at DESC
             LIMIT ?",
        ) {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        stmt.query_map(rusqlite::params![limit as i64], |row| {
            Ok(ConversationMessage {
                delegation_id: row.get(0)?,
                role: row.get(1)?,
                content: row.get(2)?,
                created_at: row.get::<_, i64>(3)? as u64,
            })
        })
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    // ── 个人记忆写入 ──

    /// 任务创建时插入 personal_memory 行（summary 留空，待产出时补全）
    pub fn memory_add_task(
        &self,
        delegation_id: &str,
        agent_id: &str,
        from_agent: &str,
        description: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        conn.execute(
            "INSERT INTO personal_memory (timestamp, agent_id, from_agent, delegation_id, description, summary, artifacts, tags)
             VALUES (?, ?, ?, ?, ?, '', '[]', '[]')",
            rusqlite::params![now as i64, agent_id, from_agent, delegation_id, description],
        )?;
        Ok(())
    }

    /// 任务产出时补全 personal_memory 的 detail
    pub fn memory_add_detail(&self, delegation_id: &str, detail: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE personal_memory SET summary = ? WHERE delegation_id = ?",
            rusqlite::params![detail, delegation_id],
        )?;
        Ok(())
    }
}

// ── Worklog trait ────────────────────────────────────────────

impl Worklog for SqliteMemory {
    fn recent(&self, limit: usize) -> Vec<WorklogEntry> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT timestamp, agent_id, delegation_id, to_agent, description, summary, reply, artifacts
             FROM worklog
             ORDER BY timestamp DESC
             LIMIT ?",
        ) {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        stmt.query_map(rusqlite::params![limit as i64], row_to_worklog)
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    fn search(&self, keyword: &str) -> Vec<WorklogEntry> {
        let conn = self.conn.lock().unwrap();
        let pattern = format!("%{}%", keyword);
        let mut stmt = match conn.prepare(
            "SELECT timestamp, agent_id, delegation_id, to_agent, description, summary, reply, artifacts
             FROM worklog
             WHERE summary LIKE ? OR agent_id LIKE ?
             ORDER BY timestamp DESC
             LIMIT 50",
        ) {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        stmt.query_map(rusqlite::params![pattern, pattern], row_to_worklog)
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }
}

// ── PersonalMemory trait ─────────────────────────────────────

impl PersonalMemory for SqliteMemory {
    fn recall(&self, keyword: &str) -> Vec<MemoryEntry> {
        let conn = self.conn.lock().unwrap();
        let pattern = format!("%{}%", keyword);
        let mut stmt = match conn.prepare(
            "SELECT timestamp, agent_id, from_agent, delegation_id, description, summary, artifacts, tags
             FROM personal_memory
             WHERE description LIKE ? OR summary LIKE ? OR tags LIKE ?
             ORDER BY timestamp DESC
             LIMIT 50",
        ) {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        stmt.query_map(rusqlite::params![pattern, pattern, pattern], row_to_memory)
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    fn recall_by_tag(&self, tag: &str) -> Vec<MemoryEntry> {
        let conn = self.conn.lock().unwrap();
        let pattern = format!("%{}%", tag);
        let mut stmt = match conn.prepare(
            "SELECT timestamp, agent_id, from_agent, delegation_id, description, summary, artifacts, tags
             FROM personal_memory
             WHERE tags LIKE ?
             ORDER BY timestamp DESC
             LIMIT 50",
        ) {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        stmt.query_map(rusqlite::params![pattern], row_to_memory)
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }
}

// ── Row mappers ──────────────────────────────────────────────

fn row_to_worklog(row: &rusqlite::Row) -> rusqlite::Result<WorklogEntry> {
    let artifacts_str: String = row.get(7).unwrap_or_default();
    let to_agent: String = row.get(3).unwrap_or_default();
    let description: String = row.get(4).unwrap_or_default();
    let reply: String = row.get(6).unwrap_or_default();
    Ok(WorklogEntry {
        timestamp: row.get::<_, i64>(0)? as u64,
        agent_id: row.get(1)?,
        delegation_id: row.get(2)?,
        to_agent,
        description,
        summary: row.get(5)?,
        reply,
        artifacts: serde_json::from_str(&artifacts_str).unwrap_or_default(),
    })
}

fn row_to_memory(row: &rusqlite::Row) -> rusqlite::Result<MemoryEntry> {
    let artifacts_str: String = row.get(6).unwrap_or_default();
    let tags_str: String = row.get(7).unwrap_or_default();
    let from_agent: String = row.get(2).unwrap_or_default();
    Ok(MemoryEntry {
        timestamp: row.get::<_, i64>(0)? as u64,
        delegation_id: row.get(3)?,
        from_agent,
        description: row.get(4).unwrap_or_default(),
        summary: row.get(5).unwrap_or_default(),
        artifacts: serde_json::from_str(&artifacts_str).unwrap_or_default(),
        tags: serde_json::from_str(&tags_str).unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_memory_worklog_flow() {
        let db = SqliteMemory::open(":memory:").unwrap();

        // 写入：任务创建
        db.worklog_add_task("d-001", "coder", "reviewer", "写用户接口")
            .unwrap();
        // 补全：产出结果
        db.worklog_add_result("d-001", "实现了 /api/users", "").unwrap();

        // 读取
        let recent = db.recent(5);
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].delegation_id, "d-001");

        // 按 agent 查
        let by_agent = db.worklog_by_agent("coder", 5).unwrap();
        assert_eq!(by_agent.len(), 1);

        // 搜索
        let search = db.search("/api");
        assert_eq!(search.len(), 1);
    }

    #[test]
    fn sqlite_memory_personal_flow() {
        let db = SqliteMemory::open(":memory:").unwrap();

        db.memory_add_task("d-001", "coder", "manager", "写用户接口")
            .unwrap();
        db.memory_add_detail(
            "d-001",
            "用 actix-web 实现了 GET /api/users，返回 JSON 列表",
        )
        .unwrap();

        // delegation_id 直查
        let found = db.personal_by_delegation("d-001");
        assert!(found.is_some());

        // 关键词召回
        let by_keyword = db.recall("actix");
        assert_eq!(by_keyword.len(), 1);
    }

    #[test]
    fn batch_delegation_lookup() {
        let db = SqliteMemory::open(":memory:").unwrap();
        db.memory_add_task("a", "agent_a", "from_a", "task A")
            .unwrap();
        db.memory_add_detail("a", "detail A").unwrap();
        db.memory_add_task("b", "agent_b", "from_b", "task B")
            .unwrap();
        db.memory_add_detail("b", "detail B").unwrap();

        let results = db.personal_by_delegations(&["a".into(), "b".into()]);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn delegation_record_format() {
        let record = DelegationRecord {
            id: "d-001".into(),
            from_agent: "manager".into(),
            to_agent: "coder".into(),
            brief: "实现JWT".into(),
            detail: "签发和验证令牌".into(),
            parent_id: None,
            result: String::new(),
        };
        let formatted = record.format();
        assert!(formatted.contains("[委托 d-001]"));
        assert!(formatted.contains("简述: 实现JWT"));
        assert!(formatted.contains("详情: 签发和验证令牌"));
        assert!(formatted.contains("委托人: manager"));
        assert!(formatted.contains("承接人: coder"));
    }
}
