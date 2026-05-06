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
    pub summary: String,
    #[serde(default)]
    pub artifacts: Vec<String>,
}

/// 个人记忆条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub timestamp: u64,
    pub delegation_id: String,
    pub description: String,
    pub summary: String,
    #[serde(default)]
    pub artifacts: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

// ── Traits ────────────────────────────────────────────────────

/// 工作日志（团队共享）
pub trait Worklog: Send + Sync {
    fn append(&self, entry: WorklogEntry) -> Result<()>;
    fn recent(&self, limit: usize) -> Vec<WorklogEntry>;
    fn search(&self, keyword: &str) -> Vec<WorklogEntry>;
}

/// 个人记忆（每 agent 私有）
pub trait PersonalMemory: Send + Sync {
    fn remember(&self, entry: MemoryEntry) -> Result<()>;
    fn recall(&self, keyword: &str) -> Vec<MemoryEntry>;
    fn recall_by_tag(&self, tag: &str) -> Vec<MemoryEntry>;
}

/// SQLite 记忆存储
///
/// 使用 Mutex 保证线程安全。workspace 内所有成员共享同一个连接。
pub struct SqliteMemory {
    conn: Mutex<Connection>,
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
                summary TEXT NOT NULL,
                artifacts TEXT NOT NULL DEFAULT '[]'
            );
            CREATE INDEX IF NOT EXISTS idx_worklog_agent ON worklog(agent_id);
            CREATE INDEX IF NOT EXISTS idx_worklog_delegation ON worklog(delegation_id);

            CREATE TABLE IF NOT EXISTS personal_memory (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp INTEGER NOT NULL,
                agent_id TEXT NOT NULL,
                delegation_id TEXT NOT NULL UNIQUE,
                description TEXT NOT NULL DEFAULT '',
                summary TEXT NOT NULL DEFAULT '',
                artifacts TEXT NOT NULL DEFAULT '[]',
                tags TEXT NOT NULL DEFAULT '[]'
            );
            CREATE INDEX IF NOT EXISTS idx_personal_agent ON personal_memory(agent_id);
            CREATE INDEX IF NOT EXISTS idx_personal_delegation ON personal_memory(delegation_id);
            ",
        )?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    // ── 供 Workspace 使用的扩展查询 ──

    /// 查询某个 agent 的近期工作日志条目（附带 delegation_id）
    pub fn worklog_by_agent(&self, agent_id: &str, limit: usize) -> Result<Vec<WorklogEntry>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT timestamp, agent_id, delegation_id, summary, artifacts
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
            "SELECT timestamp, agent_id, delegation_id, description, summary, artifacts, tags
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
            "SELECT timestamp, agent_id, delegation_id, description, summary, artifacts, tags
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
}

// ── Worklog trait ────────────────────────────────────────────

impl Worklog for SqliteMemory {
    fn append(&self, entry: WorklogEntry) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let artifacts = serde_json::to_string(&entry.artifacts).unwrap_or_default();
        conn.execute(
            "INSERT OR REPLACE INTO worklog (timestamp, agent_id, delegation_id, summary, artifacts)
             VALUES (?, ?, ?, ?, ?)",
            rusqlite::params![
                entry.timestamp as i64,
                entry.agent_id,
                entry.delegation_id,
                entry.summary,
                artifacts,
            ],
        )?;
        Ok(())
    }

    fn recent(&self, limit: usize) -> Vec<WorklogEntry> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT timestamp, agent_id, delegation_id, summary, artifacts
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
            "SELECT timestamp, agent_id, delegation_id, summary, artifacts
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
    fn remember(&self, entry: MemoryEntry) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let artifacts = serde_json::to_string(&entry.artifacts).unwrap_or_default();
        let tags = serde_json::to_string(&entry.tags).unwrap_or_default();
        conn.execute(
            "INSERT OR REPLACE INTO personal_memory
             (timestamp, agent_id, delegation_id, description, summary, artifacts, tags)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params![
                entry.timestamp as i64,
                "", // agent_id 由上层通过 delegation_id 关联 worklog 确定
                entry.delegation_id,
                entry.description,
                entry.summary,
                artifacts,
                tags,
            ],
        )?;
        Ok(())
    }

    fn recall(&self, keyword: &str) -> Vec<MemoryEntry> {
        let conn = self.conn.lock().unwrap();
        let pattern = format!("%{}%", keyword);
        let mut stmt = match conn.prepare(
            "SELECT timestamp, agent_id, delegation_id, description, summary, artifacts, tags
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
            "SELECT timestamp, agent_id, delegation_id, description, summary, artifacts, tags
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
    let artifacts_str: String = row.get(4).unwrap_or_default();
    Ok(WorklogEntry {
        timestamp: row.get::<_, i64>(0)? as u64,
        agent_id: row.get(1)?,
        delegation_id: row.get(2)?,
        summary: row.get(3)?,
        artifacts: serde_json::from_str(&artifacts_str).unwrap_or_default(),
    })
}

fn row_to_memory(row: &rusqlite::Row) -> rusqlite::Result<MemoryEntry> {
    let artifacts_str: String = row.get(5).unwrap_or_default();
    let tags_str: String = row.get(6).unwrap_or_default();
    Ok(MemoryEntry {
        timestamp: row.get::<_, i64>(0)? as u64,
        delegation_id: row.get(2)?,
        description: row.get(3).unwrap_or_default(),
        summary: row.get(4).unwrap_or_default(),
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

        // 写入
        db.append(WorklogEntry {
            timestamp: 1705000000,
            agent_id: "coder".into(),
            delegation_id: "d-001".into(),
            summary: "实现了 /api/users".into(),
            artifacts: vec!["src/users.rs".into()],
        })
        .unwrap();

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

        db.remember(MemoryEntry {
            timestamp: 1705000001,
            delegation_id: "d-001".into(),
            description: "写用户接口".into(),
            summary: "用 actix-web 实现了 GET /api/users，返回 JSON 列表".into(),
            artifacts: vec!["src/users.rs".into()],
            tags: vec!["api".into(), "backend".into()],
        })
        .unwrap();

        // delegation_id 直查
        let found = db.personal_by_delegation("d-001");
        assert!(found.is_some());
        assert_eq!(found.unwrap().tags, vec!["api", "backend"]);

        // 按标签召回
        let by_tag = db.recall_by_tag("api");
        assert_eq!(by_tag.len(), 1);

        // 关键词召回
        let by_keyword = db.recall("actix");
        assert_eq!(by_keyword.len(), 1);
    }

    #[test]
    fn batch_delegation_lookup() {
        let db = SqliteMemory::open(":memory:").unwrap();
        db.remember(MemoryEntry {
            timestamp: 1,
            delegation_id: "a".into(),
            description: "".into(),
            summary: "A".into(),
            artifacts: vec![],
            tags: vec![],
        })
        .unwrap();
        db.remember(MemoryEntry {
            timestamp: 2,
            delegation_id: "b".into(),
            description: "".into(),
            summary: "B".into(),
            artifacts: vec![],
            tags: vec![],
        })
        .unwrap();

        let results = db.personal_by_delegations(&["a".into(), "b".into()]);
        assert_eq!(results.len(), 2);
    }
}
