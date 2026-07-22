//! EventBus — 事件通道注册表
//!
//! 集中管理所有 workspace 的广播通道，用命名前缀区分 workspace。
//! 命名格式：`<workspace>/<用途>`
//!
//! 示例：
//! - `"demo/stream"` — workspace 统一事件流
//! - `"demo/task/abc-123"` — per-task 通道
//! - `"staging/stream"` — 另一个 workspace

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::RwLock;
use tokio::sync::broadcast;

const CHANNEL_CAPACITY: usize = 32;

/// 事件总线，管理所有命名通道
pub struct EventBus {
    channels: RwLock<HashMap<String, broadcast::Sender<String>>>,
}

impl EventBus {
    /// 创建新的 EventBus
    pub fn new() -> Self {
        Self {
            channels: RwLock::new(HashMap::new()),
        }
    }

    /// 注册新通道，返回接收端
    ///
    /// 如果通道已存在，返回新的接收端（不重复创建）
    pub fn register(&self, name: &str) -> broadcast::Receiver<String> {
        let mut channels = self.channels.write().unwrap();
        let tx = channels
            .entry(name.to_string())
            .or_insert_with(|| broadcast::channel(CHANNEL_CAPACITY).0);
        tx.subscribe()
    }

    /// 订阅已有通道
    ///
    /// 如果通道不存在，返回 None
    pub fn subscribe(&self, name: &str) -> Option<broadcast::Receiver<String>> {
        let channels = self.channels.read().unwrap();
        channels.get(name).map(|tx| tx.subscribe())
    }

    /// 发送消息到指定通道
    ///
    /// 返回接收者数量。如果通道不存在返回 Err。
    /// 如果发送失败（通道满/无接收者），记录警告并返回 Ok(0)。
    pub fn send(&self, name: &str, data: String) -> Result<usize> {
        let channels = self.channels.read().unwrap();
        let tx = channels
            .get(name)
            .with_context(|| format!("channel '{}' not found", name))?;

        match tx.send(data) {
            Ok(count) => Ok(count),
            Err(_) => {
                tracing::warn!("failed to send to channel '{}' (no receivers)", name);
                Ok(0)
            }
        }
    }

    /// 移除通道
    ///
    /// 返回通道是否存在。如果通道不存在返回 false。
    pub fn unregister(&self, name: &str) -> bool {
        let mut channels = self.channels.write().unwrap();
        channels.remove(name).is_some()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_and_send() {
        let bus = EventBus::new();
        let mut rx = bus.register("test/stream");

        let count = bus.send("test/stream", "hello".to_string()).unwrap();
        assert_eq!(count, 1);

        let msg = rx.try_recv().unwrap();
        assert_eq!(msg, "hello");
    }

    #[test]
    fn test_subscribe_existing() {
        let bus = EventBus::new();
        let _rx1 = bus.register("test/stream");
        let mut rx2 = bus.subscribe("test/stream").unwrap();

        bus.send("test/stream", "world".to_string()).unwrap();
        assert_eq!(rx2.try_recv().unwrap(), "world");
    }

    #[test]
    fn test_subscribe_nonexistent() {
        let bus = EventBus::new();
        assert!(bus.subscribe("nonexistent").is_none());
    }

    #[test]
    fn test_send_to_nonexistent() {
        let bus = EventBus::new();
        let result = bus.send("nonexistent", "data".to_string());
        assert!(result.is_err());
    }

    #[test]
    fn test_unregister() {
        let bus = EventBus::new();
        bus.register("test/stream");

        assert!(bus.unregister("test/stream"));
        assert!(!bus.unregister("test/stream")); // 第二次返回 false
    }

    #[test]
    fn test_multiple_receivers() {
        let bus = EventBus::new();
        let mut rx1 = bus.register("test/stream");
        let mut rx2 = bus.subscribe("test/stream").unwrap();
        let mut rx3 = bus.subscribe("test/stream").unwrap();

        let count = bus.send("test/stream", "broadcast".to_string()).unwrap();
        assert_eq!(count, 3);

        assert_eq!(rx1.try_recv().unwrap(), "broadcast");
        assert_eq!(rx2.try_recv().unwrap(), "broadcast");
        assert_eq!(rx3.try_recv().unwrap(), "broadcast");
    }
}
