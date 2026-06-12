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
}

/// Workspace 运行时
pub struct Workspace {
    pub name: String,
    pub board: Arc<DelegationBoard>,
    pub sandbox: Arc<RwLock<SandboxManager>>,
    pub db: Arc<SqliteMemory>,
    pub system_prompt: String,
    pub member_profiles: Vec<MemberProfile>,

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
