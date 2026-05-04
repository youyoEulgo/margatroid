//! Bridge 工作轮询主循环

use crate::api::BridgeApiClient;
use std::time::Duration;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};
use types::bridge::{BridgeError, WorkResponse};

/// 轮询配置
#[derive(Clone)]
pub struct PollConfig {
    pub interval_ms_not_at_capacity: u64,
    pub interval_ms_at_capacity: u64,
    pub heartbeat_interval_ms: u64,
    pub reclaim_older_than_ms: u64,
}

impl Default for PollConfig {
    fn default() -> Self {
        Self {
            interval_ms_not_at_capacity: 2000,
            interval_ms_at_capacity: 600_000,
            heartbeat_interval_ms: 0,
            reclaim_older_than_ms: 5000,
        }
    }
}

/// 轮询结果
pub enum PollEvent {
    Work(WorkResponse),
    Empty,
    EnvironmentLost,
    Fatal(BridgeError),
}

pub struct PollLoopState {
    pub environment_id: String,
    pub environment_secret: String,
    pub consecutive_errors: u32,
    pub first_error_time: Option<std::time::Instant>,
}

impl PollLoopState {
    pub fn new(environment_id: String, environment_secret: String) -> Self {
        Self {
            environment_id,
            environment_secret,
            consecutive_errors: 0,
            first_error_time: None,
        }
    }
}

/// 执行单次轮询
pub async fn poll_once(
    client: &mut BridgeApiClient,
    state: &PollLoopState,
    config: &PollConfig,
    signal: &CancellationToken,
) -> PollEvent {
    if signal.is_cancelled() {
        return PollEvent::Fatal(BridgeError::Other("cancelled".into()));
    }

    match client
        .poll_for_work(
            &state.environment_id,
            &state.environment_secret,
            Some(config.reclaim_older_than_ms),
        )
        .await
    {
        Ok(Some(work)) => PollEvent::Work(work),
        Ok(None) => PollEvent::Empty,
        Err(e) => match e.status() {
            Some(404) => {
                warn!("poll_once: environment not found (404)");
                PollEvent::EnvironmentLost
            }
            Some(status) if e.is_expired() || matches!(status, 401 | 403 | 410) => {
                PollEvent::Fatal(e)
            }
            _ => {
                warn!("poll_once: transient error: {e}");
                PollEvent::Fatal(e)
            }
        },
    }
}

/// 指数退避睡眠（带 jitter）
pub async fn backoff_sleep(attempt: u32, base_ms: u64, cap_ms: u64, signal: &CancellationToken) {
    let ms = (base_ms * 2u64.saturating_pow(attempt.saturating_sub(1))).min(cap_ms);
    let jitter = (ms as f64 * 0.25 * (rand_f64() * 2.0 - 1.0)) as i64;
    let actual = ((ms as i64) + jitter).max(0) as u64;
    debug!("backoff_sleep: {}ms (attempt {})", actual, attempt);

    tokio::select! {
        _ = sleep(Duration::from_millis(actual)) => {}
        _ = signal.cancelled() => {}
    }
}

fn rand_f64() -> f64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::time::SystemTime;
    let mut h = DefaultHasher::new();
    SystemTime::now().hash(&mut h);
    std::thread::current().id().hash(&mut h);
    (h.finish() & 0xFFFFFF) as f64 / 0xFFFFFF as f64
}
