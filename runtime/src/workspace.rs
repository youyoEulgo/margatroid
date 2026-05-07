//! Workspace — 团队协作容器
//!
//! 持有所有成员、委托板、沙箱、和 SQLite 记忆。
//! 负责上下文注入（worklog 索引 → delegation_id → personal memory）
//! 和成员控制循环。

use anyhow::Result;
use sandbox::SandboxManager;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use types::{ComposeFile, RequestTool};

use crate::board::{DelegationBoard, PRIORITY_USER};
use crate::member::Member;
use crate::memory::{SqliteMemory, Worklog};

/// 默认注入上下文的工具集
fn default_tools() -> Vec<RequestTool> {
    base_tools()
}

fn base_tools() -> Vec<RequestTool> {
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
                description: Some(
                    "将子任务委托给团队中的另一个成员（发布到委托板，异步获取结果）".into(),
                ),
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

fn manager_tools() -> Vec<RequestTool> {
    let mut tools = base_tools();
    tools.extend(vec![
        types::RequestTool {
            r#type: "function".into(),
            function: types::FunctionDescription {
                name: "schedule_add".into(),
                description: Some("向计划表添加任务（Manager 专用）".into()),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "target": { "type": "string", "description": "指派给谁" },
                        "description": { "type": "string", "description": "任务描述" },
                        "priority": { "type": "integer", "description": "优先级，越大越高" }
                    },
                    "required": ["target", "description"]
                }),
            },
        },
        types::RequestTool {
            r#type: "function".into(),
            function: types::FunctionDescription {
                name: "schedule_list".into(),
                description: Some("列出计划表所有待处理任务（Manager 专用）".into()),
                parameters: serde_json::json!({ "type": "object", "properties": {} }),
            },
        },
        types::RequestTool {
            r#type: "function".into(),
            function: types::FunctionDescription {
                name: "schedule_pop".into(),
                description: Some("为指定成员弹出计划表的下一个任务（Manager 专用）".into()),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "target": { "type": "string", "description": "成员 ID" }
                    },
                    "required": ["target"]
                }),
            },
        },
        types::RequestTool {
            r#type: "function".into(),
            function: types::FunctionDescription {
                name: "schedule_remove".into(),
                description: Some("从计划表删除任务（Manager 专用）".into()),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "integer", "description": "条目 ID" }
                    },
                    "required": ["id"]
                }),
            },
        },
    ]);
    tools
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
    pub async fn start(
        compose: &ComposeFile,
        provider: Arc<dyn providers::DynAiProvider>,
    ) -> Result<Self> {
        // 1. 沙箱
        let sandbox_config = load_sandbox_config(compose);
        let mut sandbox_mgr = SandboxManager::new();
        sandbox_mgr.initialize(sandbox_config).await?;
        let sandbox = Arc::new(RwLock::new(sandbox_mgr));

        // 2. SQLite 记忆
        let db_path = memory_path(compose);
        let db = Arc::new(SqliteMemory::open(&db_path)?);

        // 3. 委托板（持有 SQLite 引用）
        let board = Arc::new(DelegationBoard::new(db.clone()));

        // 4. 构建成员（Arc 包装以跨 task 共享）
        let mut members: HashMap<String, Arc<Member>> = HashMap::new();
        for agent in &compose.agents {
            let member = Arc::new(Member::new(
                &agent.id,
                &agent.model,
                provider.clone(),
                sandbox.clone(),
                board.clone(),
            ));
            members.insert(agent.id.clone(), member);
        }

        // 5. 启动成员循环（manager 有 schedule 工具 + 推任务逻辑）
        let mut handles = Vec::new();
        for agent in &compose.agents {
            let member = Arc::clone(&members[&agent.id]);
            let board_clone = board.clone();
            let db_clone = db.clone();
            let system_prompt = agent.system_prompt.clone();
            let is_manager = agent.id == "manager";
            let tools = if is_manager {
                manager_tools()
            } else {
                default_tools()
            };

            handles.push(tokio::spawn(async move {
                if is_manager {
                    manager_loop(member, system_prompt, board_clone, db_clone, tools).await;
                } else {
                    member_loop(member, system_prompt, board_clone, db_clone, tools).await;
                }
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
            .offer(
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
    system_prompt: String,
    board: Arc<DelegationBoard>,
    db: Arc<SqliteMemory>,
    tools: Vec<RequestTool>,
) {
    tracing::info!("成员 '{}' 启动控制循环", member.id);

    loop {
        // 0. 检查返回区——接受自己发布的任务结果
        check_and_accept_returned(&member.id, &board).await;

        // 1. claim 新任务
        let task = match board.claim(&member.id).await {
            Ok(Some(t)) => t,
            Ok(None) => {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                continue;
            }
            Err(e) => {
                tracing::error!("成员 '{}' claim 失败: {}", member.id, e);
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                continue;
            }
        };

        tracing::info!(
            "成员 '{}' 领取任务 '{}': {}",
            member.id,
            task.id,
            task.description
        );

        // 2. 准备上下文：worklog + 相关 personal memory
        let context = prepare_context(&db, &member.id, &task.description);

        // 3. 拼完整 prompt：系统 prompt + 上下文 + 任务
        let prompt = format!(
            "{}\n\n---\n\n团队工作日志与相关记忆：\n{}\n\n---\n\n任务描述：{}\n参数：{}",
            system_prompt, context, task.description, task.parameters
        );

        // 3. 执行
        let outcome = match member.chat(&prompt, &tools).await {
            Ok(o) => o,
            Err(e) => {
                let err_msg = format!("执行失败: {}", e);
                tracing::error!("成员 '{}': {}", member.id, err_msg);
                let _ = board
                    .return_task(&member.id, &task.id, &err_msg, "执行失败")
                    .await;
                continue;
            }
        };

        // 4. 提交结果到返回区（board 内部写入档案）
        if let Err(e) = board
            .return_task(&member.id, &task.id, &outcome.result, &outcome.summary)
            .await
        {
            tracing::error!("成员 '{}' 提交结果失败: {}", member.id, e);
            continue;
        }
        tracing::info!("成员 '{}' 完成任务 '{}'", member.id, task.id);
    }
}

/// Manager 专用循环：完成归档后从 schedule 推任务到发布区
async fn manager_loop(
    member: Arc<Member>,
    system_prompt: String,
    board: Arc<DelegationBoard>,
    db: Arc<SqliteMemory>,
    tools: Vec<RequestTool>,
) {
    tracing::info!("Manager '{}' 启动控制循环", member.id);

    loop {
        // 0. 检查返回区，接受的委托对应阶段任务归档
        let returned = board.check_return(&member.id).await;
        for task in &returned {
            match board.accept(&member.id, &task.id).await {
                Ok(t) => {
                    tracing::info!("Manager 接受 '{}': {}", t.id, t.result.as_deref().unwrap_or(""));
                    // 阶段任务完成 → 归档
                    board.schedule_archive_by_target(&t.to);
                }
                Err(e) => {
                    tracing::error!("Manager accept 失败 '{}': {}", task.id, e);
                }
            }
        }

        let task = match board.claim(&member.id).await {
            Ok(Some(t)) => t,
            Ok(None) => {
                push_from_schedule(&board).await;
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                continue;
            }
            Err(e) => {
                tracing::error!("Manager claim 失败: {}", e);
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                continue;
            }
        };

        tracing::info!("Manager 领取任务 '{}': {}", task.id, task.description);

        let context = prepare_context(&db, &member.id, &task.description);
        let prompt = format!(
            "{}\n\n---\n\n团队工作日志与相关记忆：\n{}\n\n---\n\n任务描述：{}\n参数：{}",
            system_prompt, context, task.description, task.parameters
        );

        let outcome = match member.chat(&prompt, &tools).await {
            Ok(o) => o,
            Err(e) => {
                let err_msg = format!("执行失败: {}", e);
                let _ = board
                    .return_task(&member.id, &task.id, &err_msg, "执行失败")
                    .await;
                continue;
            }
        };

        if let Err(e) = board
            .return_task(&member.id, &task.id, &outcome.result, &outcome.summary)
            .await
        {
            tracing::error!("Manager 提交结果失败: {}", e);
            continue;
        }
        tracing::info!("Manager 完成任务 '{}'", task.id);

        push_from_schedule(&board).await;
    }
}

/// 从计划表为每个空闲成员推阶段任务到发布区（不归档）
async fn push_from_schedule(board: &DelegationBoard) {
    let entries = board.schedule_list();
    for entry in entries {
        if board.has_offered_schedule(&entry.target) {
            continue; // 该成员的阶段任务还在执行中
        }
        if let Some(s) = board.schedule_pop(&entry.target) {
            match board
                .offer(
                    "manager",
                    &entry.target,
                    &entry.description,
                    serde_json::json!({}),
                    entry.priority as u32,
                )
                .await
            {
                Ok(_) => {} // 阶段任务已发布为委托，归档在 Manager accept 时
                Err(e) => {
                    tracing::error!("Manager offer 失败: {}, 回退条目 {}", e, s.id);
                    board.schedule_revert(s.id);
                }
            }
        }
    }
}

/// 检查返回区，自动 accept 自己发布的任务结果
async fn check_and_accept_returned(member_id: &str, board: &DelegationBoard) {
    let returned = board.check_return(member_id).await;
    for task in &returned {
        match board.accept(member_id, &task.id).await {
            Ok(t) => {
                tracing::info!(
                    "成员 '{}' 接受委托结果 '{}': {}",
                    member_id,
                    t.id,
                    t.result.as_deref().unwrap_or("")
                );
            }
            Err(e) => {
                tracing::error!("成员 '{}' accept 失败 '{}': {}", member_id, task.id, e);
            }
        }
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

// ── 帮助函数 ──────────────────────────────────────────────────

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
}
