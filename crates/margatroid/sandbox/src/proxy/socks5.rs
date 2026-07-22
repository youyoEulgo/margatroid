//! SOCKS5 代理（Phase 3 完整实现）
//!
//! 当前为骨架——代理服务器拦截非 HTTP TCP 流量（SSH、数据库连接等）
//! 并按域名白名单过滤。

use anyhow::Result;

/// SOCKS5 代理服务器（骨架）
#[derive(Debug)]
pub struct Socks5Proxy {
    port: u16,
}

impl Socks5Proxy {
    pub fn new(port: u16) -> Self {
        Self { port }
    }

    /// 启动代理（当前无操作）
    pub async fn start(&mut self) -> Result<()> {
        tracing::info!("SOCKS5 proxy stub listening on port {}", self.port);
        Ok(())
    }

    /// 停止代理
    pub async fn stop(&mut self) -> Result<()> {
        tracing::info!("SOCKS5 proxy stopped");
        Ok(())
    }

    /// 获取代理端口
    pub fn port(&self) -> u16 {
        self.port
    }
}
