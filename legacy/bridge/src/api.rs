//! Bridge HTTP API 客户端
//!
//! 与具体 AI 服务商无关，只处理 Bridge/Environment 层协议。
//! 认证信息通过 `BridgeApiConfig` 注入，不硬编码任何服务商地址。

use crate::work_secret::validate_bridge_id;
use anyhow::Result;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use types::bridge::{BridgeConfig, BridgeError, PermissionResponseEvent, WorkResponse};

const BETA_HEADER: &str = "environments-2025-11-01";

/// API 客户端配置（依赖注入，不绑定任何服务商）
#[derive(Clone)]
pub struct BridgeApiConfig {
    pub base_url: String,
    pub runner_version: String,
    /// 认证回调，每次请求时调用获取最新 token
    pub get_access_token: std::sync::Arc<dyn Fn() -> Option<String> + Send + Sync>,
    pub trusted_device_token: Option<String>,
}

pub struct BridgeApiClient {
    client: Client,
    config: BridgeApiConfig,
    consecutive_empty_polls: u64,
}

const EMPTY_POLL_LOG_INTERVAL: u64 = 100;

impl BridgeApiClient {
    pub fn new(config: BridgeApiConfig) -> Self {
        Self {
            client: Client::new(),
            config,
            consecutive_empty_polls: 0,
        }
    }

    fn headers(&self, token: &str) -> reqwest::header::HeaderMap {
        let mut map = reqwest::header::HeaderMap::new();
        map.insert(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        map.insert(
            reqwest::header::CONTENT_TYPE,
            "application/json".parse().unwrap(),
        );
        map.insert("anthropic-version", "2023-06-01".parse().unwrap());
        map.insert("anthropic-beta", BETA_HEADER.parse().unwrap());
        map.insert(
            "x-environment-runner-version",
            self.config.runner_version.parse().unwrap(),
        );
        if let Some(dt) = &self.config.trusted_device_token {
            map.insert("X-Trusted-Device-Token", dt.parse().unwrap());
        }
        map
    }

    fn resolve_auth(&self) -> Result<String, BridgeError> {
        (self.config.get_access_token)()
            .ok_or_else(|| BridgeError::Config("No access token available".into()))
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.config.base_url.trim_end_matches('/'))
    }

    fn handle_status(status: StatusCode, body: &str, context: &str) -> Result<(), BridgeError> {
        if status.is_success() {
            return Ok(());
        }
        let detail = extract_error_detail(body);
        let error_type = extract_error_type(body);

        match status.as_u16() {
            401 => Err(BridgeError::fatal_with_type(
                format!("{context}: Authentication failed (401). {detail:?}"),
                401,
                error_type.unwrap_or_default(),
            )),
            403 => {
                let msg = if is_expired(&error_type) {
                    "Session expired. Please restart remote-control.".into()
                } else {
                    format!("{context}: Access denied (403). {detail:?}")
                };
                Err(BridgeError::fatal_with_type(
                    msg,
                    403,
                    error_type.unwrap_or_default(),
                ))
            }
            404 => Err(BridgeError::fatal_with_type(
                detail.unwrap_or_else(|| format!("{context}: Not found (404)")),
                404,
                error_type.unwrap_or_default(),
            )),
            410 => Err(BridgeError::fatal_with_type(
                detail.unwrap_or_else(|| "Session expired.".into()),
                410,
                error_type.unwrap_or("environment_expired".into()),
            )),
            429 => Err(BridgeError::Other(format!("{context}: Rate limited (429)"))),
            s => Err(BridgeError::Other(format!(
                "{context}: Failed with status {s}. {detail:?}"
            ))),
        }
    }

    /// 注册 bridge 环境，返回 (environment_id, environment_secret)
    pub async fn register_bridge_environment(
        &self,
        config: &BridgeConfig,
    ) -> Result<(String, String), BridgeError> {
        let token = self.resolve_auth()?;
        let url = self.url("/v1/environments/bridge");

        #[derive(Serialize)]
        struct Body<'a> {
            machine_name: &'a str,
            directory: &'a str,
            branch: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            git_repo_url: Option<&'a str>,
            max_sessions: u32,
            metadata: serde_json::Value,
            #[serde(skip_serializing_if = "Option::is_none")]
            environment_id: Option<&'a str>,
        }

        #[derive(Deserialize)]
        struct Resp {
            environment_id: String,
            environment_secret: String,
        }

        let body = Body {
            machine_name: &config.machine_name,
            directory: &config.dir,
            branch: &config.branch,
            git_repo_url: config.git_repo_url.as_deref(),
            max_sessions: config.max_sessions,
            metadata: serde_json::json!({ "worker_type": config.worker_type }),
            environment_id: config.reuse_environment_id.as_deref(),
        };

        let resp = self
            .client
            .post(&url)
            .headers(self.headers(&token))
            .json(&body)
            .timeout(std::time::Duration::from_secs(15))
            .send()
            .await?;

        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        Self::handle_status(status, &text, "Registration")?;

        let r: Resp = serde_json::from_str(&text)
            .map_err(|e| BridgeError::Other(format!("Registration parse error: {e}")))?;
        Ok((r.environment_id, r.environment_secret))
    }

    /// 轮询工作队列，返回 None 表示当前没有工作
    pub async fn poll_for_work(
        &mut self,
        environment_id: &str,
        environment_secret: &str,
        reclaim_older_than_ms: Option<u64>,
    ) -> Result<Option<WorkResponse>, BridgeError> {
        validate_bridge_id(environment_id, "environmentId")?;

        let prev_empty = self.consecutive_empty_polls;
        self.consecutive_empty_polls = 0;

        let mut url = self.url(&format!("/v1/environments/{environment_id}/work/poll"));
        if let Some(ms) = reclaim_older_than_ms {
            url.push_str(&format!("?reclaim_older_than_ms={ms}"));
        }

        let resp = self
            .client
            .get(&url)
            .headers(self.headers(environment_secret))
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await?;

        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        Self::handle_status(status, &text, "Poll")?;

        if text.trim().is_empty() || text.trim() == "null" {
            self.consecutive_empty_polls = prev_empty + 1;
            if self.consecutive_empty_polls == 1
                || self.consecutive_empty_polls % EMPTY_POLL_LOG_INTERVAL == 0
            {
                tracing::debug!(
                    "poll_for_work: no work ({} consecutive empty polls)",
                    self.consecutive_empty_polls
                );
            }
            return Ok(None);
        }

        let work: WorkResponse = serde_json::from_str(&text)
            .map_err(|e| BridgeError::Other(format!("Poll parse error: {e}")))?;
        Ok(Some(work))
    }

    /// 确认工作项已接收
    pub async fn acknowledge_work(
        &self,
        environment_id: &str,
        work_id: &str,
        session_token: &str,
    ) -> Result<(), BridgeError> {
        validate_bridge_id(environment_id, "environmentId")?;
        validate_bridge_id(work_id, "workId")?;

        let url = self.url(&format!(
            "/v1/environments/{environment_id}/work/{work_id}/ack"
        ));
        let resp = self
            .client
            .post(&url)
            .headers(self.headers(session_token))
            .json(&serde_json::json!({}))
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await?;

        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        Self::handle_status(status, &text, "Acknowledge")
    }

    /// 停止工作项
    pub async fn stop_work(
        &self,
        environment_id: &str,
        work_id: &str,
        force: bool,
    ) -> Result<(), BridgeError> {
        validate_bridge_id(environment_id, "environmentId")?;
        validate_bridge_id(work_id, "workId")?;

        let token = self.resolve_auth()?;
        let url = self.url(&format!(
            "/v1/environments/{environment_id}/work/{work_id}/stop"
        ));
        let resp = self
            .client
            .post(&url)
            .headers(self.headers(&token))
            .json(&serde_json::json!({ "force": force }))
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await?;

        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        Self::handle_status(status, &text, "StopWork")
    }

    /// 注销环境
    pub async fn deregister_environment(&self, environment_id: &str) -> Result<(), BridgeError> {
        validate_bridge_id(environment_id, "environmentId")?;

        let token = self.resolve_auth()?;
        let url = self.url(&format!("/v1/environments/bridge/{environment_id}"));
        let resp = self
            .client
            .delete(&url)
            .headers(self.headers(&token))
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await?;

        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        Self::handle_status(status, &text, "Deregister")
    }

    /// 归档 session
    pub async fn archive_session(&self, session_id: &str) -> Result<(), BridgeError> {
        validate_bridge_id(session_id, "sessionId")?;

        let token = self.resolve_auth()?;
        let url = self.url(&format!("/v1/sessions/{session_id}/archive"));
        let resp = self
            .client
            .post(&url)
            .headers(self.headers(&token))
            .json(&serde_json::json!({}))
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await?;

        let status = resp.status();
        // 409 表示已归档，幂等处理
        if status.as_u16() == 409 {
            return Ok(());
        }
        let text = resp.text().await.unwrap_or_default();
        Self::handle_status(status, &text, "ArchiveSession")
    }

    /// 重连 session（让服务端重新分派工作）
    pub async fn reconnect_session(
        &self,
        environment_id: &str,
        session_id: &str,
    ) -> Result<(), BridgeError> {
        validate_bridge_id(environment_id, "environmentId")?;
        validate_bridge_id(session_id, "sessionId")?;

        let token = self.resolve_auth()?;
        let url = self.url(&format!(
            "/v1/environments/{environment_id}/bridge/reconnect"
        ));
        let resp = self
            .client
            .post(&url)
            .headers(self.headers(&token))
            .json(&serde_json::json!({ "session_id": session_id }))
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await?;

        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        Self::handle_status(status, &text, "ReconnectSession")
    }

    /// 发送心跳，返回 (lease_extended, state)
    pub async fn heartbeat_work(
        &self,
        environment_id: &str,
        work_id: &str,
        session_token: &str,
    ) -> Result<(bool, String), BridgeError> {
        validate_bridge_id(environment_id, "environmentId")?;
        validate_bridge_id(work_id, "workId")?;

        let url = self.url(&format!(
            "/v1/environments/{environment_id}/work/{work_id}/heartbeat"
        ));

        #[derive(Deserialize)]
        struct Resp {
            lease_extended: bool,
            state: String,
        }

        let resp = self
            .client
            .post(&url)
            .headers(self.headers(session_token))
            .json(&serde_json::json!({}))
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await?;

        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        Self::handle_status(status, &text, "Heartbeat")?;

        let r: Resp = serde_json::from_str(&text)
            .map_err(|e| BridgeError::Other(format!("Heartbeat parse error: {e}")))?;
        Ok((r.lease_extended, r.state))
    }

    /// 发送权限响应事件到 session
    pub async fn send_permission_response_event(
        &self,
        session_id: &str,
        event: &PermissionResponseEvent,
        session_token: &str,
    ) -> Result<(), BridgeError> {
        validate_bridge_id(session_id, "sessionId")?;

        let url = self.url(&format!("/v1/sessions/{session_id}/events"));
        let resp = self
            .client
            .post(&url)
            .headers(self.headers(session_token))
            .json(&serde_json::json!({ "events": [event] }))
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await?;

        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        Self::handle_status(status, &text, "SendPermissionResponseEvent")
    }
}

// ── 辅助函数 ──────────────────────────────────────────────

fn extract_error_detail(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    v.get("message")
        .and_then(|m| m.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            v.get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .map(|s| s.to_string())
        })
}

fn extract_error_type(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    v.get("error")
        .and_then(|e| e.get("type"))
        .and_then(|t| t.as_str())
        .map(|s| s.to_string())
}

fn is_expired(error_type: &Option<String>) -> bool {
    error_type
        .as_deref()
        .map(|t| t.contains("expired") || t.contains("lifetime"))
        .unwrap_or(false)
}
