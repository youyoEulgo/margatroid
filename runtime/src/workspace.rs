//! Workspace — 团队协作容器
//!
//! 持有成员、委托板、沙箱、SQLite 记忆。
//! 成员由调用方构造传入，Workspace 不负责创建。
//! 负责上下文注入和成员控制循环。

use anyhow::Result;
use sandbox::SandboxManager;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use types::{ComposeFile, RequestTool};

use crate::agent::Agent;
use crate::board::{DelegationBoard, PRIORITY_USER};
use crate::memory::{SqliteMemory, Worklog};

pub fn base_tools() -> Vec<RequestTool> {
    vec![
        types::RequestTool {
            r#type: "function".into(),
            function: types::FunctionDescription {
                name: "bash".into(),
                description: Some("在沙箱环境中执行 shell 命令".into()),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "要执行的 shell 命令" }
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
                        "target": { "type": "string", "description": "目标成员 ID" },
                        "task": { "type": "string", "description": "任务描述" },
                        "priority": { "type": "integer", "description": "优先级，数字越大越优先，默认 0" }
                    },
                    "required": ["target", "task"]
                }),
            },
        },
        types::RequestTool {
            r#type: "function".into(),
            function: types::FunctionDescription {
                name: "delegate_reject".into(),
                description: Some("驳回收到的委托结果".into()),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "task_id": { "type": "string", "description": "要驳回的委托 ID" }
                    },
                    "required": ["task_id"]
                }),
            },
        },
        types::RequestTool {
            r#type: "function".into(),
            function: types::FunctionDescription {
                name: "recall".into(),
                description: Some("搜索工作日志和个人记忆".into()),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "keyword": { "type": "string", "description": "搜索关键词" }
                    },
                    "required": ["keyword"]
                }),
            },
        },
        types::RequestTool {
            r#type: "function".into(),
            function: types::FunctionDescription {
                name: "finish".into(),
                description: Some("完成当前委托并返回结果".into()),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "summary": { "type": "string", "description": "完成摘要" },
                        "result": { "type": "string", "description": "详细结果" }
                    },
                    "required": ["summary"]
                }),
            },
        },
    ]
}

pub fn manager_tools() -> Vec<RequestTool> {
    let mut tools = base_tools();
    tools.extend(vec![
        types::RequestTool {
            r#type: "function".into(),
            function: types::FunctionDescription {
                name: "schedule_add".into(),
                description: Some("向计划表添加阶段任务".into()),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "target": { "type": "string", "description": "指派给谁" },
                        "description": { "type": "string", "description": "任务描述" },
                        "priority": { "type": "integer", "description": "优先级" }
                    },
                    "required": ["target", "description"]
                }),
            },
        },
        types::RequestTool {
            r#type: "function".into(),
            function: types::FunctionDescription {
                name: "schedule_list".into(),
                description: Some("列出计划表".into()),
                parameters: serde_json::json!({ "type": "object", "properties": {} }),
            },
        },
        types::RequestTool {
            r#type: "function".into(),
            function: types::FunctionDescription {
                name: "schedule_pop".into(),
                description: Some("为指定成员弹出下一个阶段任务".into()),
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
                description: Some("从计划表删除任务".into()),
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

/// 传给 Workspace 的成员配置
pub struct AgentEntry {
    pub agent: Arc<dyn Agent>,
    pub system_prompt: String,
    pub tools: Vec<RequestTool>,
}

pub struct Workspace {
    pub board: Arc<DelegationBoard>,
    pub sandbox: Arc<RwLock<SandboxManager>>,
    pub db: Arc<SqliteMemory>,
    members: HashMap<String, Arc<dyn Agent>>,
    handles: Vec<tokio::task::JoinHandle<()>>,
}

impl Workspace {
    pub async fn start(
        compose: &ComposeFile,
        entries: Vec<AgentEntry>,
        provider: Arc<dyn providers::DynAiProvider>,
    ) -> Result<Self> {
        let sandbox_config = load_sandbox_config(compose);
        let mut sandbox_mgr = SandboxManager::new();
        sandbox_mgr.initialize(sandbox_config).await?;
        let sandbox = Arc::new(RwLock::new(sandbox_mgr));

        let db_path = memory_path(compose);
        let db = Arc::new(SqliteMemory::open(&db_path)?);

        let board = Arc::new(DelegationBoard::new(db.clone()));

        let mut members: HashMap<String, Arc<dyn Agent>> = HashMap::new();
        let mut handles = Vec::new();

        for entry in entries {
            let member_id = entry.agent.id().to_string();
            members.insert(member_id.clone(), entry.agent.clone());

            let entry_agent = entry.agent;
            let board_clone = board.clone();
            let db_clone = db.clone();
            let system_prompt = entry.system_prompt;
            let tools = entry.tools;

            handles.push(tokio::spawn(async move {
                member_loop(entry_agent, system_prompt, board_clone, db_clone, tools).await;
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

    pub async fn send_user_message(
        &self,
        from: &str,
        to: &str,
        brief: &str,
        detail: &str,
    ) -> Result<String> {
        self.board
            .offer(from, to, brief, detail, None, PRIORITY_USER)
            .await
    }

    pub fn member(&self, id: &str) -> Option<&Arc<dyn Agent>> {
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

/// 审核自己发出去的委托 — 向下插入
async fn review_delegations(
    agent: &dyn Agent,
    board: &DelegationBoard,
    system_prompt: &str,
    tools: &[RequestTool],
) {
    let returned = board.check_return(agent.id()).await;
    if returned.is_empty() {
        return;
    }

    // 将每条返回的委托格式化后向下插入上级委托的 result
    for t in &returned {
        let child_formatted = format!("\n---\n{}", t.format());
        if let Some(ref parent_id) = t.parent_id {
            let _ = board.append_result(parent_id, &child_formatted);
        }
    }

    // 构建审核提示词
    let items: Vec<String> = returned.iter().map(|t| t.format()).collect();
    let review_prompt = format!(
        "{}\n\n---\n\n审核你发出去的委托结果：\n{}\n\n满意的默认通过。不满意的调 delegate_reject(task_id) 驳回。",
        system_prompt,
        items.join("\n\n")
    );
    let _ = agent.process(&review_prompt, "审核委托", tools).await;

    for t in &board.check_return(agent.id()).await {
        let _ = board.accept(agent.id(), &t.id).await;
    }
}

/// 处理别人发来的委托 — 向上查
async fn execute_task(
    agent: &dyn Agent,
    board: &DelegationBoard,
    db: &SqliteMemory,
    system_prompt: &str,
    tools: &[RequestTool],
) {
    let task = match board.claim(agent.id()).await {
        Ok(Some(t)) => t,
        _ => return,
    };

    tracing::info!(
        "成员 '{}' 领取任务 '{}': {}",
        agent.id(),
        task.id,
        task.brief
    );

    // 构建委托链上下文：沿 parent_id 向上追溯
    let chain_context = build_chain(board, &task);

    let memory_context = prepare_context(db, agent.id(), &task.brief);
    let prompt = format!(
        "{}\n\n---\n\n团队工作日志与相关记忆：\n{}\n\n---\n\n委托链上下文：\n{}\n\n---\n\n当前任务：{}",
        system_prompt, memory_context, chain_context, task.brief
    );

    let outcome = match agent.process(&prompt, &task.brief, tools).await {
        Ok(o) => o,
        Err(e) => {
            let _ = board
                .return_task(agent.id(), &task.id, &e.to_string(), "执行失败")
                .await;
            return;
        }
    };
    let _ = board
        .return_task(agent.id(), &task.id, &outcome.result, &outcome.summary)
        .await;
}

fn build_chain(board: &DelegationBoard, task: &crate::board::DelegationTask) -> String {
    // 沿 parent_id 向上收集所有祖先
    let mut ancestors: Vec<String> = Vec::new();
    let mut current_pid = task.parent_id.clone();
    while let Some(pid) = current_pid {
        match board.get_task(&pid) {
            Some(record) => {
                current_pid = record.parent_id.clone();
                ancestors.push(record.format());
            }
            None => break,
        }
    }
    // 反转：祖先 → 子孙
    ancestors.reverse();
    // 追加当前任务
    ancestors.push(task.format());
    ancestors.join("\n")
}

async fn member_loop(
    agent: Arc<dyn Agent>,
    system_prompt: String,
    board: Arc<DelegationBoard>,
    db: Arc<SqliteMemory>,
    tools: Vec<RequestTool>,
) {
    tracing::info!("成员 '{}' 启动控制循环", agent.id());

    loop {
        review_delegations(&*agent, &board, &system_prompt, &tools).await;
        execute_task(&*agent, &board, &db, &system_prompt, &tools).await;
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

fn prepare_context(db: &SqliteMemory, member_id: &str, task_description: &str) -> String {
    let mut parts = Vec::new();

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
                    &m.summary,
                    m.tags.join(", ")
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        parts.push(format!("你的相关记忆：\n{}", memory_text));
    }

    if !task_description.is_empty() {
        let keyword_hits = db.search(task_description);
        if !keyword_hits.is_empty() {
            let related = keyword_hits
                .iter()
                .take(5)
                .map(|e| format!("[{}] {}", e.delegation_id, &e.summary))
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
    let local = std::path::PathBuf::from(".margatroid")
        .join("workspace")
        .join(&compose.workspace.name)
        .join("memory.db");
    let _ = std::fs::create_dir_all(local.parent().unwrap());
    local.to_string_lossy().to_string()
}

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
