//! 容量唤醒原语

use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

pub struct CapacityWake {
    /// 外部 shutdown 信号（只读，不由本模块取消）
    outer: CancellationToken,
    /// 可替换的容量唤醒 token
    inner: Arc<Mutex<CancellationToken>>,
}

impl CapacityWake {
    pub fn new(outer: CancellationToken) -> Self {
        Self {
            outer,
            inner: Arc::new(Mutex::new(CancellationToken::new())),
        }
    }

    /// 触发一次容量唤醒：取消旧 token，替换为新 token
    pub fn wake(&self) {
        let mut guard = self.inner.lock().unwrap();
        guard.cancel();
        *guard = CancellationToken::new();
    }

    /// 获取合并信号（outer || inner）
    ///
    /// 返回的 token 在 outer 或 inner 任一取消时触发。
    pub fn signal(&self) -> CancellationToken {
        let inner_token = self.inner.lock().unwrap().clone();
        let merged = CancellationToken::new();

        // 监听 outer
        {
            let merged_clone = merged.clone();
            let outer_clone = self.outer.clone();
            tokio::spawn(async move {
                outer_clone.cancelled().await;
                merged_clone.cancel();
            });
        }

        // 监听 inner
        {
            let merged_clone = merged.clone();
            tokio::spawn(async move {
                inner_token.cancelled().await;
                merged_clone.cancel();
            });
        }

        // 如果已经取消则立即触发
        if self.outer.is_cancelled() || self.inner.lock().unwrap().is_cancelled() {
            merged.cancel();
        }

        merged
    }
}
