use anyhow::{Result, bail};
use providers::DynAiProvider;
use runtime;
use serde::Serialize;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{Mutex, RwLock};

use crate::factory;

/// 对外暴露的 provider 信息（不包含 api_key 等敏感字段）
#[derive(Debug, Clone, Serialize)]
pub struct ProviderInfo {
    pub name: String,
    pub provider_type: String,
    pub enabled: bool,
    pub base_url: String,
    pub models: Vec<String>,
    /// 是否已成功加载并可用
    pub ready: bool,
}

pub type DynProvider = Arc<dyn DynAiProvider>;

#[derive(Clone)]
pub struct AppState {
    providers: Arc<RwLock<HashMap<String, DynProvider>>>,
    pub config_mgr: Arc<Mutex<assets::Manager>>,
    workspaces: Arc<RwLock<HashMap<String, Arc<runtime::Workspace>>>>,
}

impl AppState {
    pub async fn new(config_mgr: assets::Manager) -> Result<Self> {
        let app_config = config_mgr.app_config().clone();
        let mut providers = HashMap::new();
        for p in &app_config.ai.providers {
            if !p.enabled {
                continue;
            }
            match factory::build(p) {
                Ok(inst) => {
                    tracing::info!("provider '{}' ready", p.name);
                    providers.insert(p.name.clone(), inst);
                }
                Err(e) => {
                    tracing::error!("provider '{}' build failed: {}", p.name, e);
                }
            }
        }
        Ok(Self {
            providers: Arc::new(RwLock::new(providers)),
            config_mgr: Arc::new(Mutex::new(config_mgr)),
            workspaces: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    pub async fn get_provider(&self, name: &str) -> Option<DynProvider> {
        let guard = self.providers.read().await;
        guard.get(name).cloned()
    }

    /// 查询当前配置的所有 AI provider，包含运行时加载状态
    pub async fn list_providers(&self) -> Vec<ProviderInfo> {
        let cfg_guard = self.config_mgr.lock().await;
        let providers_guard = self.providers.read().await;
        cfg_guard
            .app_config()
            .ai
            .providers
            .iter()
            .map(|p| ProviderInfo {
                name: p.name.clone(),
                provider_type: p.provider_type.clone(),
                enabled: p.enabled,
                base_url: p.base_url.clone(),
                models: p.models.clone(),
                ready: providers_guard.contains_key(&p.name),
            })
            .collect()
    }

    pub async fn reload(&self) -> Result<()> {
        let mgr = self.config_mgr.lock().await;
        let app_config = mgr.app_config().clone();
        drop(mgr);
        let mut new_providers = HashMap::new();
        for p in &app_config.ai.providers {
            if !p.enabled {
                continue;
            }
            match factory::build(p) {
                Ok(inst) => {
                    new_providers.insert(p.name.clone(), inst);
                }
                Err(e) => {
                    tracing::error!("reload: provider '{}' build failed: {}", p.name, e);
                }
            }
        }
        let mut guard = self.providers.write().await;
        *guard = new_providers;
        tracing::info!("runtime reload done");
        Ok(())
    }

    // ── Workspace ──────────────────────────────────────────────

    /// 获取 workspace 引用
    pub async fn workspace(&self, name: &str) -> Option<Arc<runtime::Workspace>> {
        self.workspaces.read().await.get(name).cloned()
    }

    /// 启动 workspace 并注册到服务器
    pub async fn start_workspace(
        &self,
        compose: &types::ComposeFile,
        entries: Vec<runtime::AgentEntry>,
    ) -> Result<()> {
        let name = compose.workspace.name.clone();
        {
            let guard = self.workspaces.read().await;
            if guard.contains_key(&name) {
                bail!("workspace '{}' is already running", name);
            }
        }

        let ws = Arc::new(runtime::Workspace::start(compose, entries).await?);

        // 后台轮询日志
        let ws_clone = ws.clone();
        let ws_name = name.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                let status = ws_clone.board.status().await;
                tracing::info!(
                    "workspace '{}' board: publish={}",
                    ws_name,
                    status.publish_count,
                );
            }
        });

        self.workspaces.write().await.insert(name, ws);
        Ok(())
    }
}

pub struct AnyhowError(pub anyhow::Error);
impl<E: Into<anyhow::Error>> From<E> for AnyhowError {
    fn from(e: E) -> Self {
        Self(e.into())
    }
}
impl axum::response::IntoResponse for AnyhowError {
    fn into_response(self) -> axum::response::Response {
        let body = serde_json::json!({ "error": self.0.to_string() });
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            axum::response::Json(body),
        )
            .into_response()
    }
}
