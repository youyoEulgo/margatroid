//! Workspace — 团队协作容器
//!
//! 持有成员、委托板、任务链、SQLite 记忆。
//! 成员由调用方构造传入，Workspace 不负责创建。
//! 负责任务链管理和成员控制循环。

use anyhow::Result;
use sandbox::SandboxManager;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use types::{ComposeFile, RequestTool};

use crate::board::DelegationBoard;
use crate::member::Agent;
use crate::memory::SqliteMemory;

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
                        "task_summary": { "type": "string", "description": "委托出去的任务简述" },
                        "task_detail": { "type": "string", "description": "委托出去的任务详细描述" },
                        "work_summary": { "type": "string", "description": "一句话简述发委托前做了什么、产出什么、有什么缺漏" },
                        "work_detail": { "type": "string", "description": "发委托前干的事情的具体的思路、做法" }
                    },
                    "required": ["target", "task_summary", "task_detail", "work_summary", "work_detail"]
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
                description: Some("完成当前委托，产出最终结果".into()),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "summary": { "type": "string", "description": "一句话简述完成委托做了什么、产出什么" },
                        "detail": { "type": "string", "description": "完成委托的具体思路、做法" }
                    },
                    "required": ["summary", "detail"]
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
    pub soul: String,
    pub tools: Vec<RequestTool>,
    pub skills: Vec<String>,
}

pub struct Workspace {
    pub board: Arc<DelegationBoard>,
    pub sandbox: Arc<RwLock<SandboxManager>>,
    pub db: Arc<SqliteMemory>,
    members: HashMap<String, Arc<dyn Agent>>,
    handles: Vec<tokio::task::JoinHandle<()>>,
    shutdown: CancellationToken,
}

impl Workspace {
    pub async fn start(compose: &ComposeFile, entries: Vec<AgentEntry>) -> Result<Self> {
        let sandbox_config = load_sandbox_config(compose);
        let mut sandbox_mgr = SandboxManager::new();
        sandbox_mgr.initialize(sandbox_config).await?;
        let sandbox = Arc::new(RwLock::new(sandbox_mgr));

        let db_path = memory_path(compose);
        let db = Arc::new(SqliteMemory::open(&db_path)?);

        let mut member_profiles = Vec::new();
        for entry in &entries {
            let id = entry.agent.id().to_string();
            let identity = entry.agent.identity();
            let label = match identity {
                types::Identity::Manager => "经理",
                types::Identity::Member => "成员",
                types::Identity::User => "用户",
            }
            .to_string();
            member_profiles.push((id, label, entry.skills.clone()));
        }

        let board = Arc::new(DelegationBoard::new(
            db.clone(),
            compose.workspace.system_prompt.clone(),
            member_profiles,
        ));
        let shutdown = CancellationToken::new();

        let mut members: HashMap<String, Arc<dyn Agent>> = HashMap::new();
        let mut handles = Vec::new();

        for entry in entries {
            let member_id = entry.agent.id().to_string();
            let agent = entry.agent.clone();
            members.insert(member_id, agent.clone());

            let board_clone = board.clone();
            let tools = entry.tools;
            let shutdown_clone = shutdown.clone();

            handles.push(tokio::spawn(async move {
                member_loop(agent, board_clone, tools, shutdown_clone).await;
            }));
        }

        Ok(Self {
            board,
            sandbox,
            db,
            members,
            handles,
            shutdown,
        })
    }

    /// 发送用户消息：创建根委托并排入发布区
    pub async fn send_user_message(
        &self,
        from: &str,
        to: &str,
        brief: &str,
        detail: &str,
    ) -> Result<String> {
        self.board.offer(from, to, brief, detail, None).await
    }

    pub fn member(&self, id: &str) -> Option<&Arc<dyn Agent>> {
        self.members.get(id)
    }

    /// 优雅关闭：通知所有成员退出并等待完成
    pub async fn shutdown(mut self) {
        self.shutdown.cancel();
        let handles = std::mem::take(&mut self.handles);
        for handle in handles {
            let _ = handle.await;
        }
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

/// 处理任务：读链 → 匹配成员 → process → handle outcome
async fn execute_task(agent: &dyn Agent, board: &DelegationBoard, tools: &[RequestTool]) {
    const MAX_RETRIES: u32 = 3;

    // 从任务链读取当前委托
    let task_chain = board.chain_snapshot().await;
    let task = match task_chain.current_task().cloned() {
        Some(t) if t.to == agent.id() && !t.id.is_empty() => t,
        _ => return,
    };

    if task_chain.has_outcome() {
        tracing::info!("processing | {} → {} | {}", task.from, task.to, task.brief,);
    } else {
        tracing::info!("delegation | {} → {} | {}", task.from, task.to, task.brief,);
    }

    let did = task.id.clone();

    board
        .trigger_event(
            types::event_index::EVENT_MEMBER_STATUS,
            &task.id,
            &format!("{}\nworking", agent.id()),
        )
        .await;

    match agent.process(board, tools).await {
        Ok(outcome) => {
            tracing::debug!(
                "成员 '{}' 完成委托 '{}': {}",
                agent.id(),
                task.id,
                outcome.result
            );
        }
        Err(e) => {
            if let Err(e) = board
                .result(
                    agent.id(),
                    crate::board::TaskResult {
                        delegation_id: task.id.clone(),
                        detail: e.to_string(),
                        summary: "执行失败".into(),
                        done: false,
                        reply: String::new(),
                    },
                )
                .await
            {
                tracing::error!("记录失败结果出错: {}", e);
            }

            // 提取 / 递增重试计数
            let (retries, inner_detail) = parse_retry(&task.detail);
            if retries >= MAX_RETRIES {
                tracing::warn!("任务 '{}' 已达最大重试次数 {}，放弃", task.id, MAX_RETRIES);
                return;
            }
            let detail = format!("[RETRY:{}] {}", retries + 1, inner_detail);

            if let Err(e) = board
                .offer(
                    &task.from,
                    &task.to,
                    &task.brief,
                    &detail,
                    task.parent_id.as_deref(),
                )
                .await
            {
                tracing::error!("重新发布失败: {}", e);
            }
        }
    };
    board
        .trigger_event(
            types::event_index::EVENT_MEMBER_STATUS,
            &did,
            &format!("{}\nidle", agent.id()),
        )
        .await;
}

fn parse_retry(detail: &str) -> (u32, &str) {
    if let Some(rest) = detail.strip_prefix("[RETRY:") {
        if let Some(idx) = rest.find(']') {
            if let Ok(n) = rest[..idx].parse::<u32>() {
                return (n, rest[idx + 1..].trim());
            }
        }
    }
    (0, detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_retry_no_prefix() {
        let (n, detail) = parse_retry("some task detail");
        assert_eq!(n, 0);
        assert_eq!(detail, "some task detail");
    }

    #[test]
    fn parse_retry_first() {
        let (n, detail) = parse_retry("[RETRY:1] failed: timeout");
        assert_eq!(n, 1);
        assert_eq!(detail, "failed: timeout");
    }

    #[test]
    fn parse_retry_third() {
        let (n, detail) = parse_retry("[RETRY:3] something broke");
        assert_eq!(n, 3);
        assert_eq!(detail, "something broke");
    }

    #[test]
    fn parse_retry_malformed_no_bracket() {
        let (n, detail) = parse_retry("[RETRY:2");
        assert_eq!(n, 0);
        assert_eq!(detail, "[RETRY:2");
    }

    #[test]
    fn parse_retry_malformed_not_a_number() {
        let (n, detail) = parse_retry("[RETRY:two] error");
        assert_eq!(n, 0);
        assert_eq!(detail, "[RETRY:two] error");
    }
}

async fn member_loop(
    agent: Arc<dyn Agent>,
    board: Arc<DelegationBoard>,
    tools: Vec<RequestTool>,
    shutdown: CancellationToken,
) {
    tracing::info!("成员 '{}' 启动控制循环", agent.id());

    loop {
        if shutdown.is_cancelled() {
            break;
        }
        execute_task(&*agent, &board, &tools).await;
        tokio::select! {
            _ = board.wait(agent.id()) => {},
            _ = shutdown.cancelled() => {},
        }
    }
    tracing::info!("成员 '{}' 控制循环退出", agent.id());
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
