use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AppInfo {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ConfigServer {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ConfigLogging {
    pub level: String,
    pub format: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AiConfig {
    pub timeout_secs: u64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AiProvider {
    pub name: String,
    pub provider_type: String,
    pub enabled: bool,
    pub base_url: String,
    pub api_key: String,
    pub models: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ConfigAi {
    pub config: AiConfig,
    pub providers: Vec<AiProvider>,
}

/// 全局配置
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AppConfig {
    pub app: AppInfo,
    pub server: ConfigServer,
    pub logging: ConfigLogging,
    pub ai: ConfigAi,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            app: AppInfo {
                name: "Margatroid".into(),
                version: "0.1.0".into(),
            },
            server: ConfigServer {
                host: "127.0.0.1".into(),
                port: 3939,
            },
            logging: ConfigLogging {
                level: "INFO".into(),
                format: "json".into(),
            },
            ai: ConfigAi {
                config: AiConfig { timeout_secs: 60 },
                providers: [].to_vec(),
            },
        }
    }
}

// ------------------------

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WorkspaceInfo {
    pub version: String,
    pub description: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AiRequest {
    pub model: String,
    pub temperature: f32,
    pub top_p: f32,
    pub stream: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WorkspaceAi {
    pub request: AiRequest,
}

/// 工作区配置
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WorkspaceConfig {
    pub workspace: WorkspaceInfo,
    pub ai: WorkspaceAi,
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        Self {
            workspace: WorkspaceInfo {
                version: "0.1.0".into(),
                description: "".into(),
            },
            ai: WorkspaceAi {
                request: AiRequest {
                    model: "".into(),
                    temperature: 0.9,
                    top_p: 0.2,
                    stream: true,
                },
            },
        }
    }
}
