//! HTTP/HTTPS 代理（Phase 3 完整实现）
//!
//! 当前为骨架——代理服务器拦截 HTTP 请求并按域名白名单过滤。
//! 沙箱内进程通过 Unix Domain Socket（Linux）或 localhost 端口（macOS）
//! 连接到本代理。

use anyhow::Result;

/// HTTP 代理服务器（骨架）
#[derive(Debug)]
pub struct HttpProxy {
    port: u16,
}

impl HttpProxy {
    pub fn new(port: u16) -> Self {
        Self { port }
    }

    /// 启动代理（当前无操作）
    pub async fn start(&mut self) -> Result<()> {
        tracing::info!("HTTP proxy stub listening on port {}", self.port);
        Ok(())
    }

    /// 停止代理
    pub async fn stop(&mut self) -> Result<()> {
        tracing::info!("HTTP proxy stopped");
        Ok(())
    }

    /// 获取代理端口
    pub fn port(&self) -> u16 {
        self.port
    }
}
