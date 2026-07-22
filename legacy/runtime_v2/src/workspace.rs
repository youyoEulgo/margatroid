//! Workspace V2 — 单个 workspace 的运行时状态
//!
//! 职责：
//! - 创建并持有 board、db、sandbox、members
//! - 接收 EventBus（从 Kernel 传入）
//! - 启动成员循环
//! - 提供用户消息入口

use anyhow::Result;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use types::{MemberProfile, RequestTool};

use crate::board::DelegationBoard;
use crate::events::EventBus;
use crate::member::Agent;
use crate::memory::SqliteMemory;

use sandbox::SandboxManager;

/// 传给 Workspace 的成员配置
pub struct AgentEntry {
    pub agent: Arc<dyn Agent>,
    pub soul: String,
    pub tools: Vec<RequestTool>,
    pub skills: Vec<String>,
    // V2 兼容字段：用于在 Workspace 内部重新构造 Member
    pub client: Option<providers::Client>,
    pub sandbox: Option<Arc<RwLock<SandboxManager>>>,
}

/// Workspace 运行时
pub struct Workspace {
    pub name: String,
    pub board: Arc<DelegationBoard>,
    pub sandbox: Arc<RwLock<SandboxManager>>,
    pub db: Arc<SqliteMemory>,
    pub system_prompt: String,
    pub member_profiles: Vec<MemberProfile>,
    pub event_bus: Arc<EventBus>,

    handles: Vec<tokio::task::JoinHandle<()>>,
    shutdown: CancellationToken,
}

impl Workspace {
    /// 创建新的 Workspace 并启动所有成员循环
    pub async fn new(
        name: String,
        system_prompt: String,
        member_profiles: Vec<MemberProfile>,
        entries: Vec<AgentEntry>,
        event_bus: Arc<EventBus>,
        db_path: std::path::PathBuf,
    ) -> Result<Self> {
        let db = Arc::new(SqliteMemory::open(&db_path.to_string_lossy())?);

        let member_ids: Vec<String> = member_profiles.iter().map(|p| p.id.clone()).collect();
        let board = Arc::new(DelegationBoard::new(db.clone(), member_ids));

        let sandbox = Arc::new(RwLock::new(SandboxManager::new()));

        event_bus.register(&format!("{}/stream", name));

        let shutdown = CancellationToken::new();
        let mut handles = Vec::new();

        for entry in entries {
            let agent = entry.agent;
            let board = board.clone();
            let tools = entry.tools;
            let event_bus = event_bus.clone();
            let workspace_name = name.clone();
            let system_prompt = system_prompt.clone();
            let member_profiles = member_profiles.clone();
            let shutdown = shutdown.clone();

            handles.push(tokio::spawn(async move {
                crate::engine::member_loop(
                    agent,
                    board,
                    tools,
                    event_bus,
                    workspace_name,
                    system_prompt,
                    member_profiles,
                    shutdown,
                )
                .await;
            }));
        }

        Ok(Self {
            name,
            board,
            sandbox,
            db,
            system_prompt,
            member_profiles,
            event_bus,
            handles,
            shutdown,
        })
    }

    pub async fn send_user_message(
        &self,
        from: &str,
        to: &str,
        brief: &str,
        detail: &str,
    ) -> Result<String> {
        self.board.offer(from, to, brief, detail, None).await
    }

    pub async fn status(&self) -> crate::board::BoardStatus {
        self.board.status().await
    }

    pub async fn shutdown(self) {
        self.shutdown.cancel();
        for handle in self.handles {
            let _ = handle.await;
        }
        tracing::info!("workspace '{}' shutdown", self.name);
    }

    /// 兼容层：V1 风格的 start() 方法
    /// 从 ComposeFile 提取参数并调用 new()
    pub async fn start(
        compose: &types::ComposeFile,
        entries: Vec<AgentEntry>,
    ) -> Result<Self> {
        // 提取配置
        let name = compose.workspace.name.clone();
        let system_prompt = compose.workspace.system_prompt.clone();

        // 创建全局 EventBus（V1 兼容模式，每个 workspace 独立 EventBus）
        let event_bus = Arc::new(EventBus::new());

        // 构造 member_profiles 并重新构造 Member（注入 event_bus 和 workspace_name）
        let mut member_profiles = Vec::new();
        let mut new_entries = Vec::new();

        for entry in entries {
            let id = entry.agent.id().to_string();
            let identity = entry.agent.identity();
            let display_name = match identity {
                types::Identity::Manager => "经理",
                types::Identity::Member => "成员",
                types::Identity::User => "用户",
            }
            .to_string();
            member_profiles.push(types::MemberProfile {
                id: id.clone(),
                display_name,
                tags: entry.skills.clone(),
            });

            // 重新构造 Member（如果提供了 client 和 sandbox）
            let new_agent: Arc<dyn Agent> = if let (Some(client), Some(sandbox)) = (entry.client, entry.sandbox) {
                Arc::new(crate::member::Member::new(
                    &id,
                    entry.soul.clone(),
                    identity.clone(),
                    client,
                    sandbox,
                    event_bus.clone(),
                    name.clone(),
                ))
            } else {
                // 没有提供构造参数，直接使用原 agent（可能是测试用的 mock）
                entry.agent
            };

            new_entries.push(AgentEntry {
                agent: new_agent,
                soul: entry.soul,
                tools: entry.tools,
                skills: entry.skills,
                client: None,
                sandbox: None,
            });
        }

        // 构造 db_path（与 v1 保持一致）
        let db_path = std::path::PathBuf::from(".margatroid")
            .join("workspace")
            .join(&name)
            .join("memory.db");

        // 确保父目录存在
        if let Some(parent) = db_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        Self::new(name, system_prompt, member_profiles, new_entries, event_bus, db_path).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_workspace_creation() {
        let event_bus = Arc::new(EventBus::new());
        let temp_db = std::env::temp_dir().join(format!("test_ws_{}.db", uuid::Uuid::new_v4()));

        let profiles = vec![MemberProfile {
            id: "alice".to_string(),
            display_name: "Alice".to_string(),
            tags: vec!["测试".to_string()],
        }];

        let ws = Workspace::new(
            "test".to_string(),
            "系统提示".to_string(),
            profiles,
            vec![],
            event_bus.clone(),
            temp_db,
        )
        .await
        .unwrap();

        assert_eq!(ws.name, "test");
        assert_eq!(ws.system_prompt, "系统提示");
        assert!(event_bus.subscribe("test/stream").is_some());
    }

    #[tokio::test]
    async fn test_workspace_send_message() {
        let event_bus = Arc::new(EventBus::new());
        let temp_db = std::env::temp_dir().join(format!("test_ws_{}.db", uuid::Uuid::new_v4()));

        let profiles = vec![MemberProfile {
            id: "alice".to_string(),
            display_name: "Alice".to_string(),
            tags: vec![],
        }];

        let ws = Workspace::new(
            "test".to_string(),
            String::new(),
            profiles,
            vec![],
            event_bus,
            temp_db,
        )
        .await
        .unwrap();

        let task_id = ws
            .send_user_message("user", "alice", "测试任务", "详细内容")
            .await
            .unwrap();

        assert!(!task_id.is_empty());

        let status = ws.status().await;
        assert_eq!(status.publish_count, 1);
    }
}
