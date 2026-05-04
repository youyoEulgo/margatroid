//! JWT Token 刷新调度器

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error};

pub type RefreshCallback = Arc<dyn Fn(String) + Send + Sync>;

pub struct RefreshConfig {
    pub buffer_secs: u64,
    pub retry_interval_secs: u64,
    pub max_failures: u32,
}

impl Default for RefreshConfig {
    fn default() -> Self {
        Self {
            buffer_secs: 5 * 60,
            retry_interval_secs: 60,
            max_failures: 3,
        }
    }
}

/// 解析 JWT 的 exp 字段（不验证签名）
pub fn decode_jwt_expiry(token: &str) -> Option<u64> {
    let jwt = if token.starts_with("sk-ant-si-") {
        &token["sk-ant-si-".len()..]
    } else {
        token
    };

    let parts: Vec<&str> = jwt.split('.').collect();
    if parts.len() != 3 {
        return None;
    }

    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    let decoded = URL_SAFE_NO_PAD
        .decode(parts[1])
        .or_else(|_| {
            let pad = (4 - parts[1].len() % 4) % 4;
            let padded = format!("{}{}", parts[1], "=".repeat(pad));
            base64::engine::general_purpose::STANDARD.decode(&padded)
        })
        .ok()?;

    let payload: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    payload.get("exp")?.as_u64()
}

struct SchedulerState {
    timer: Option<tokio::task::JoinHandle<()>>,
    failure_count: u32,
}

pub struct TokenRefreshScheduler {
    config: RefreshConfig,
    on_refresh: RefreshCallback,
    states: Arc<Mutex<std::collections::HashMap<String, SchedulerState>>>,
}

impl TokenRefreshScheduler {
    pub fn new(config: RefreshConfig, on_refresh: RefreshCallback) -> Self {
        Self {
            config,
            on_refresh,
            states: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    pub async fn schedule(&self, session_id: String, token: &str, shutdown: CancellationToken) {
        let expiry = match decode_jwt_expiry(token) {
            Some(e) => e,
            None => {
                debug!("schedule: cannot decode expiry for {session_id}");
                return;
            }
        };

        self.cancel(&session_id).await;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let delay_secs = expiry
            .saturating_sub(now)
            .saturating_sub(self.config.buffer_secs);

        debug!("schedule: token for {session_id} refreshes in {delay_secs}s");

        let states = Arc::clone(&self.states);
        let callback = Arc::clone(&self.on_refresh);
        let sid = session_id.clone();
        let retry_interval = self.config.retry_interval_secs;
        let max_failures = self.config.max_failures;

        let handle = tokio::spawn(async move {
            if delay_secs > 0 {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(delay_secs)) => {}
                    _ = shutdown.cancelled() => return,
                }
            }

            callback(sid.clone());

            let mut failures = 0u32;
            loop {
                tokio::time::sleep(Duration::from_secs(retry_interval)).await;
                {
                    let guard = states.lock().await;
                    if let Some(s) = guard.get(&sid) {
                        failures = s.failure_count;
                    }
                }
                if failures >= max_failures {
                    error!("token refresh: max failures for {sid}");
                    break;
                }
                callback(sid.clone());
            }
        });

        let mut guard = self.states.lock().await;
        guard.insert(
            session_id,
            SchedulerState {
                timer: Some(handle),
                failure_count: 0,
            },
        );
    }

    pub async fn cancel(&self, session_id: &str) {
        let mut guard = self.states.lock().await;
        if let Some(state) = guard.remove(session_id) {
            if let Some(handle) = state.timer {
                handle.abort();
            }
        }
    }

    pub async fn cancel_all(&self) {
        let mut guard = self.states.lock().await;
        for (_, state) in guard.drain() {
            if let Some(handle) = state.timer {
                handle.abort();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_jwt_expiry() {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        let payload = r#"{"exp":9999999999}"#;
        let encoded = URL_SAFE_NO_PAD.encode(payload);
        let fake_jwt = format!("header.{encoded}.signature");
        assert_eq!(decode_jwt_expiry(&fake_jwt), Some(9999999999));
    }

    #[test]
    fn test_decode_jwt_invalid() {
        assert!(decode_jwt_expiry("not-a-jwt").is_none());
        assert!(decode_jwt_expiry("").is_none());
    }
}
