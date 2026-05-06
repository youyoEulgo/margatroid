//! Workspace — 团队协作容器
//!
//! 持有所有成员、委托板、沙箱、和 SQLite 记忆。
//! 负责上下文注入（worklog 索引 → delegation_id → personal memory）
//! 和成员控制循环。

use anyhow::Result;
use sandbox::SandboxManager;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use types::{ComposeFile, RequestTool};

use crate::board::{DelegationBoard, PRIORITY_USER};
use crate::member::{DynProviderLike, Member};
use crate::memory::{PersonalMemory, SqliteMemory, Worklog};

/// 默认注入上下文的工具集
fn default_tools() -> Vec<RequestTool> {
    vec![
        types::RequestTool {
            r#type: "function".into(),
            function: types::FunctionDescription {
                name: "bash".into(),
                description: Some("在沙箱环境中执行 shell 命令".into()),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "要执行的 shell 命令"
                        }
                    },
                    "required": ["command"]
                }),
            },
        },
        types::RequestTool {
            r#type: "function".into(),
            function: types::FunctionDescription {
                name: "delegate".into(),
                description: Some("将子任务委托给团队中的另一个成员".into()),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "target": {
                            "type": "string",
                            "description": "目标成员 ID"
                        },
                        "task": {
                            "type": "string",
                            "description": "任务描述"
                        }
                    },
                    "required": ["target", "task"]
                }),
            },
        },
    ]
}

/// Workspace —— 所有成员的运行环境
pub struct Workspace {
    pub board: Arc<DelegationBoard>,
    pub sandbox: Arc<RwLock<SandboxManager>>,
    pub db: Arc<SqliteMemory>,
    members: HashMap<String, Arc<Member>>,
    handles: Vec<tokio::task::JoinHandle<()>>,
}

impl Workspace {
    /// 从 compose 文件创建并启动所有成员
    pub async fn start(compose: &ComposeFile, provider: Arc<dyn DynProviderLike>) -> Result<Self> {
        // 1. 沙箱
        let sandbox_config = load_sandbox_config(compose);
        let mut sandbox_mgr = SandboxManager::new();
        sandbox_mgr.initialize(sandbox_config).await?;
        let sandbox = Arc::new(RwLock::new(sandbox_mgr));

        // 2. 委托板
        let board = Arc::new(DelegationBoard::new());

        // 3. SQLite 记忆
        let db_path = memory_path(compose);
        let db = Arc::new(SqliteMemory::open(&db_path)?);

        // 4. 构建成员（Arc 包装以跨 task 共享）
        let mut members: HashMap<String, Arc<Member>> = HashMap::new();
        for agent in &compose.agents {
            let member = Arc::new(Member::new(
                &agent.id,
                &agent.model,
                &agent.system_prompt,
                provider.clone(),
                sandbox.clone(),
            ));
            members.insert(agent.id.clone(), member);
        }

        // 5. 启动成员循环
        let tools = default_tools();
        let mut handles = Vec::new();
        for (_id, member) in &members {
            let member = Arc::clone(member);
            let board_clone = board.clone();
            let db_clone = db.clone();
            let tools = tools.clone();

            handles.push(tokio::spawn(async move {
                member_loop(member, board_clone, db_clone, tools).await;
            }));
        }

        Ok(Self {
            board,
            sandbox,
            db,
            members,
            handles,
        })
    }

    /// 向 Workspace 发送用户消息
    pub async fn send_user_message(&self, message: &str) -> Result<String> {
        let task_id = self
            .board
            .post(
                "user",
                "manager",
                message,
                serde_json::json!({"source": "user"}),
                PRIORITY_USER,
            )
            .await?;
        Ok(task_id)
    }

    /// 获取成员引用
    pub fn member(&self, id: &str) -> Option<&Arc<Member>> {
        self.members.get(id)
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        for handle in self.handles.drain(..) {
            handle.abort();
        }
    }
}

// ── 成员控制循环 ────────────────────────────────────────────

async fn member_loop(
    member: Arc<Member>,
    board: Arc<DelegationBoard>,
    db: Arc<SqliteMemory>,
    tools: Vec<RequestTool>,
) {
    tracing::info!("成员 '{}' 启动控制循环", member.id);

    loop {
        // 1. poll
        let task = match board.poll(&member.id).await {
            Ok(Some(t)) => t,
            Ok(None) => {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                continue;
            }
            Err(e) => {
                tracing::error!("成员 '{}' poll 失败: {}", member.id, e);
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                continue;
            }
        };

        tracing::info!(
            "成员 '{}' 收到任务 '{}': {}",
            member.id,
            task.id,
            task.description
        );

        // 2. 准备上下文：worklog + 相关 personal memory
        let context = prepare_context(&db, &member.id, &task.description);

        // 3. 拼 prompt
        let prompt = format!(
            "团队工作日志与相关记忆：\n{}\n\n任务描述：{}\n参数：{}",
            context, task.description, task.parameters
        );

        // 4. 调用 member.chat()（内部 tool-call loop + 内联总结）
        let outcome = match member.chat(&prompt, &tools).await {
            Ok(o) => o,
            Err(e) => {
                let err_msg = format!("执行失败: {}", e);
                tracing::error!("成员 '{}': {}", member.id, err_msg);
                let _ = board.complete(&member.id, &task.id, &err_msg).await;
                continue;
            }
        };

        // 5. 提交结果
        if let Err(e) = board.complete(&member.id, &task.id, &outcome.result).await {
            tracing::error!("成员 '{}' 提交结果失败: {}", member.id, e);
            continue;
        }
        tracing::info!("成员 '{}' 完成任务 '{}'", member.id, task.id);

        // 6. 写记忆
        write_memory(
            &db,
            &member.id,
            &task.id,
            &task.description,
            &outcome.summary,
        );
    }
}

// ── 上下文注入 ───────────────────────────────────────────────

/// 从 worklog 索引出发，找到相关 personal memory，拼成上下文文本
fn prepare_context(db: &SqliteMemory, member_id: &str, task_description: &str) -> String {
    let mut parts = Vec::new();

    // 第一层：近期工作日志（团队 + 本人）
    let team_log = db.recent(20);
    if !team_log.is_empty() {
        let log_text = team_log
            .iter()
            .map(|e| {
                format!(
                    "[{}] {} — {}",
                    chrono_lite::timestamp_to_date(e.timestamp),
                    e.agent_id,
                    e.summary
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        parts.push(format!("团队近期工作日志：\n{}", log_text));
    }

    // 第二层：本人近期 worklog → delegation_id → 对应的 personal memory
    let my_log = db.worklog_by_agent(member_id, 10).unwrap_or_default();
    let delegation_ids: Vec<String> = my_log.iter().map(|e| e.delegation_id.clone()).collect();
    let relevant_memories = db.personal_by_delegations(&delegation_ids);

    if !relevant_memories.is_empty() {
        let memory_text = relevant_memories
            .iter()
            .map(|m| {
                format!(
                    "[{}] {} — tags: {}",
                    m.delegation_id,
                    truncate(&m.summary, 100),
                    m.tags.join(", ")
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        parts.push(format!("你的相关记忆：\n{}", memory_text));
    }

    // 第三层：关键词匹配（任务描述和 worklog summary 交集）
    if !task_description.is_empty() {
        let keyword_hits = db.search(task_description);
        if !keyword_hits.is_empty() {
            let related = keyword_hits
                .iter()
                .take(5)
                .map(|e| format!("[{}] {}", e.delegation_id, truncate(&e.summary, 80)))
                .collect::<Vec<_>>()
                .join("\n");
            parts.push(format!("关键词相关的工作记录：\n{}", related));
        }
    }

    if parts.is_empty() {
        "(暂无记录)".to_string()
    } else {
        parts.join("\n\n")
    }
}

fn truncate(s: &str, max_chars: usize) -> &str {
    if s.len() <= max_chars {
        s
    } else {
        &s[..max_chars]
    }
}

// ── 记忆写入 ─────────────────────────────────────────────────

fn write_memory(
    db: &SqliteMemory,
    agent_id: &str,
    delegation_id: &str,
    description: &str,
    summary: &str,
) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // worklog
    if let Err(e) = db.append(crate::memory::WorklogEntry {
        timestamp: now,
        agent_id: agent_id.to_string(),
        delegation_id: delegation_id.to_string(),
        summary: format!("{} — {}", description, summary),
        artifacts: vec![],
    }) {
        tracing::error!("写工作日志失败: {}", e);
    }

    // personal memory
    if let Err(e) = db.remember(crate::memory::MemoryEntry {
        timestamp: now,
        delegation_id: delegation_id.to_string(),
        description: description.to_string(),
        summary: summary.to_string(),
        artifacts: vec![],
        tags: vec![],
    }) {
        tracing::error!("写个人记忆失败: {}", e);
    }
}

// ── Helpers ──────────────────────────────────────────────────

fn load_sandbox_config(compose: &ComposeFile) -> sandbox::config::SandboxConfig {
    let workspace_name = &compose.workspace.name;
    let sandbox_path = paths::margatroid_root()
        .unwrap_or_else(|| std::path::PathBuf::from(".margatroid"))
        .join("workspace")
        .join(workspace_name)
        .join("sandbox.toml");

    let workspace_config = if sandbox_path.exists() {
        std::fs::read_to_string(&sandbox_path)
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    } else {
        sandbox::config::SandboxConfig::default()
    };

    let user_config = sandbox::load_user_config().unwrap_or_default();
    workspace_config.merge(&user_config)
}

fn memory_path(compose: &ComposeFile) -> String {
    let root = paths::margatroid_root().unwrap_or_else(|| std::path::PathBuf::from(".margatroid"));
    root.join("workspace")
        .join(&compose.workspace.name)
        .join("memory.db")
        .to_string_lossy()
        .to_string()
}

/// 极简时间戳转日期
mod chrono_lite {
    pub fn timestamp_to_date(ts: u64) -> String {
        let secs_per_day: u64 = 86400;
        let base_ts: u64 = 1704067200;
        if ts < base_ts {
            return "---".to_string();
        }
        let days = ((ts - base_ts) / secs_per_day) as i32;
        let year = 2024 + days / 365;
        let day_of_year = days % 365;
        let months = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
        let mut month = 1;
        let mut remaining = day_of_year;
        for days_in_month in &months {
            if remaining < *days_in_month {
                break;
            }
            remaining -= days_in_month;
            month += 1;
        }
        let day = remaining + 1;
        format!("{:04}-{:02}-{:02}", year, month, day)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_context_with_empty_db() {
        let db = SqliteMemory::open(":memory:").unwrap();
        let ctx = prepare_context(&db, "coder", "写一个 API");
        assert!(ctx.contains("暂无记录"));
    }

    #[test]
    fn prepare_context_with_data() {
        use crate::memory::{MemoryEntry, WorklogEntry};

        let db = SqliteMemory::open(":memory:").unwrap();
        db.append(WorklogEntry {
            timestamp: 1705000000,
            agent_id: "coder".into(),
            delegation_id: "d-001".into(),
            summary: "实现了 /api/users".into(),
            artifacts: vec!["src/users.rs".into()],
        })
        .unwrap();
        db.remember(MemoryEntry {
            timestamp: 1705000001,
            delegation_id: "d-001".into(),
            description: "写用户接口".into(),
            summary: "用 actix-web 实现 GET /api/users".into(),
            artifacts: vec![],
            tags: vec!["api".into()],
        })
        .unwrap();

        let ctx = prepare_context(&db, "coder", "修复 API bug");
        assert!(ctx.contains("/api/users"));
    }
}
