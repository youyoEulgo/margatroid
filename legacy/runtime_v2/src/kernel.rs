//! Kernel — Runtime 唯一入口
//!
//! 持有全局资源：
//! - EventBus: 事件通道注册表
//! - config_mgr: 配置管理器
//! - workspaces: workspace 列表

use crate::EventBus;
use anyhow::Result;
use assets::Manager;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use types::MemberProfile;

use crate::workspace::{AgentEntry, Workspace};

/// Kernel 是 runtime 的唯一入口
pub struct Kernel {
    /// 全局事件总线
    pub event_bus: Arc<EventBus>,
    /// 配置管理器（成员库、provider 配置等）
    pub config_mgr: Manager,
    /// workspace 列表
    workspaces: RwLock<HashMap<String, Arc<Workspace>>>,
}

impl Kernel {
    /// 创建新的 Kernel
    pub fn new(config_mgr: Manager) -> Self {
        Self {
            event_bus: Arc::new(EventBus::new()),
            config_mgr,
            workspaces: RwLock::new(HashMap::new()),
        }
    }

    /// 创建 workspace 并启动成员循环
    pub async fn create_workspace(
        &self,
        name: &str,
        system_prompt: String,
        member_profiles: Vec<MemberProfile>,
        entries: Vec<AgentEntry>,
        db_path: std::path::PathBuf,
    ) -> Result<Arc<Workspace>> {
        let workspace = Arc::new(
            Workspace::new(
                name.to_string(),
                system_prompt,
                member_profiles,
                entries,
                self.event_bus.clone(),
                db_path,
            )
            .await?,
        );

        self.workspaces
            .write()
            .unwrap()
            .insert(name.to_string(), workspace.clone());

        Ok(workspace)
    }

    /// 获取 workspace
    pub fn workspace(&self, name: &str) -> Option<Arc<Workspace>> {
        self.workspaces.read().unwrap().get(name).cloned()
    }

    /// 移除 workspace
    pub async fn remove_workspace(&self, name: &str) -> Option<Arc<Workspace>> {
        self.workspaces.write().unwrap().remove(name)
    }

    /// 列出所有 workspace 名称
    pub fn list_workspaces(&self) -> Vec<String> {
        self.workspaces
            .read()
            .unwrap()
            .keys()
            .cloned()
            .collect()
    }

    /// 关停所有 workspace
    pub async fn shutdown_all(self) {
        let workspaces = {
            let mut guard = self.workspaces.write().unwrap();
            std::mem::take(&mut *guard)
        };

        for (name, workspace) in workspaces {
            tracing::info!("shutting down workspace: {}", name);
            let ws = Arc::try_unwrap(workspace)
                .unwrap_or_else(|_| panic!("workspace still has outstanding references: {}", name));
            ws.shutdown().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paths::MargatroidPaths;

    fn test_kernel() -> Kernel {
        let temp_dir =
            std::env::temp_dir().join(format!("margatroid_test_{}", uuid::Uuid::new_v4()));
        let paths = Arc::new(MargatroidPaths::new(temp_dir));
        let config_mgr = Manager::new(paths).init().unwrap();
        Kernel::new(config_mgr)
    }

    #[tokio::test]
    async fn test_kernel_create_workspace() {
        let kernel = test_kernel();
        let db_path =
            std::env::temp_dir().join(format!("test_kernel_{}.db", uuid::Uuid::new_v4()));

        let ws = kernel
            .create_workspace("demo", String::new(), vec![], vec![], db_path)
            .await
            .unwrap();

        assert_eq!(ws.name, "demo");
        assert!(kernel.workspace("demo").is_some());
        assert_eq!(kernel.list_workspaces(), vec!["demo"]);
    }

    #[tokio::test]
    async fn test_kernel_remove_workspace() {
        let kernel = test_kernel();
        let db_path =
            std::env::temp_dir().join(format!("test_kernel_{}.db", uuid::Uuid::new_v4()));

        kernel
            .create_workspace("demo", String::new(), vec![], vec![], db_path)
            .await
            .unwrap();

        let removed = kernel.remove_workspace("demo").await;
        assert!(removed.is_some());
        assert!(kernel.workspace("demo").is_none());
    }

    #[tokio::test]
    async fn test_kernel_event_bus() {
        let kernel = test_kernel();
        let db_path =
            std::env::temp_dir().join(format!("test_kernel_{}.db", uuid::Uuid::new_v4()));

        kernel
            .create_workspace("demo", String::new(), vec![], vec![], db_path)
            .await
            .unwrap();

        // 验证 workspace 的统一事件流通道已注册
        let mut rx = kernel.event_bus.subscribe("demo/stream").unwrap();
        kernel
            .event_bus
            .send("demo/stream", "test".to_string())
            .unwrap();

        assert_eq!(rx.try_recv().unwrap(), "test");
    }
}
