//! HTTP/HTTPS 代理
//!
//! 本地 HTTP forward proxy，按域名白名单过滤请求。
//! 沙箱内进程通过 `http_proxy=http://127.0.0.1:<port>` 使用。

use anyhow::Result;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

/// HTTP 转发代理
pub struct HttpProxy {
    port: u16,
    allowed_domains: Vec<String>,
    denied_domains: Vec<String>,
    handle: Option<JoinHandle<()>>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl HttpProxy {
    pub fn new(port: u16, allowed_domains: Vec<String>, denied_domains: Vec<String>) -> Self {
        Self {
            port,
            allowed_domains,
            denied_domains,
            handle: None,
            shutdown: None,
        }
    }

    /// 启动代理服务器
    pub async fn start(&mut self) -> Result<()> {
        let listener = TcpListener::bind(format!("127.0.0.1:{}", self.port)).await?;
        tracing::info!("HTTP proxy listening on 127.0.0.1:{}", self.port);

        let allowed = self.allowed_domains.clone();
        let denied = self.denied_domains.clone();
        let (tx, mut rx) = tokio::sync::oneshot::channel::<()>();

        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    result = listener.accept() => {
                        match result {
                            Ok((stream, _addr)) => {
                                let allowed = allowed.clone();
                                let denied = denied.clone();
                                tokio::spawn(handle_connection(stream, allowed, denied));
                            }
                            Err(e) => {
                                tracing::error!("HTTP proxy accept error: {}", e);
                            }
                        }
                    }
                    _ = &mut rx => {
                        tracing::info!("HTTP proxy shutting down");
                        break;
                    }
                }
            }
        });

        self.handle = Some(handle);
        self.shutdown = Some(tx);
        Ok(())
    }

    /// 停止代理
    pub async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.await;
        }
        Ok(())
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for HttpProxy {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

/// 处理单个客户端连接
async fn handle_connection(mut client: TcpStream, allowed: Vec<String>, denied: Vec<String>) {
    let mut buf = vec![0u8; 4096];
    let n = match client.read(&mut buf).await {
        Ok(n) if n > 0 => n,
        _ => return,
    };

    let request = String::from_utf8_lossy(&buf[..n]);
    let first_line = request.lines().next().unwrap_or("");

    // CONNECT host:port HTTP/1.1 (HTTPS tunneling)
    if first_line.starts_with("CONNECT ") {
        let parts: Vec<&str> = first_line.split_whitespace().collect();
        if parts.len() >= 2 {
            let host_port = parts[1];
            let host = host_port.split(':').next().unwrap_or(host_port);

            if !is_domain_allowed(host, &allowed, &denied) {
                let _ = client.write_all(b"HTTP/1.1 403 Forbidden\r\n\r\n").await;
                tracing::warn!("HTTP proxy blocked CONNECT to: {}", host);
                return;
            }

            // Allow the CONNECT, then tunnel
            let _ = client
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .await;

            match TcpStream::connect(host_port).await {
                Ok(remote) => {
                    tunnel(client, remote).await;
                }
                Err(e) => {
                    tracing::error!("Failed to connect to {}: {}", host_port, e);
                }
            }
            return;
        }
    }

    // Regular HTTP request — extract Host header
    let host = request
        .lines()
        .find(|l| l.to_lowercase().starts_with("host:"))
        .and_then(|l| l.split(':').nth(1))
        .map(|s| s.trim())
        .unwrap_or("unknown");

    if !is_domain_allowed(host, &allowed, &denied) {
        let _ = client.write_all(b"HTTP/1.1 403 Forbidden\r\n\r\n").await;
        tracing::warn!("HTTP proxy blocked request to: {}", host);
        return;
    }

    // Forward request to remote
    let remote_addr = if host.contains(':') {
        host.to_string()
    } else {
        format!("{}:80", host)
    };

    match TcpStream::connect(&remote_addr).await {
        Ok(mut remote) => {
            let _ = remote.write_all(&buf[..n]).await;
            tunnel_bidirectional(&mut client, &mut remote).await;
        }
        Err(e) => {
            let _ = client
                .write_all(format!("HTTP/1.1 502 Bad Gateway\r\n\r\n{}", e).as_bytes())
                .await;
        }
    }
}

/// 双向隧道（CONNECT 模式）
async fn tunnel(mut client: TcpStream, mut remote: TcpStream) {
    let (mut cr, mut cw) = client.split();
    let (mut rr, mut rw) = remote.split();

    let c_to_r = tokio::io::copy(&mut cr, &mut rw);
    let r_to_c = tokio::io::copy(&mut rr, &mut cw);

    let _ = tokio::try_join!(c_to_r, r_to_c);
}

/// 双向隧道（非 CONNECT 模式，复用已读取 buffer）
async fn tunnel_bidirectional(client: &mut TcpStream, remote: &mut TcpStream) {
    let (mut cr, mut cw) = client.split();
    let (mut rr, mut rw) = remote.split();

    let c_to_r = tokio::io::copy(&mut cr, &mut rw);
    let r_to_c = tokio::io::copy(&mut rr, &mut cw);

    let _ = tokio::try_join!(c_to_r, r_to_c);
}

/// 检查域名是否在白名单中
fn is_domain_allowed(host: &str, allowed: &[String], denied: &[String]) -> bool {
    // 先检查黑名单
    for pattern in denied {
        if match_domain(host, pattern) {
            return false;
        }
    }

    // 白名单为空 = 拒绝所有
    if allowed.is_empty() {
        return false;
    }

    for pattern in allowed {
        if match_domain(host, pattern) {
            return true;
        }
    }

    false
}

/// 域名匹配（支持 *.example.com 通配符）
fn match_domain(host: &str, pattern: &str) -> bool {
    let host = host.to_lowercase();
    let pattern = pattern.to_lowercase();

    if pattern.starts_with("*.") {
        let suffix = &pattern[1..]; // ".example.com"
        host.ends_with(suffix) || host == &suffix[1..] // "example.com" matches too
    } else {
        host == pattern
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_match_exact() {
        assert!(match_domain("github.com", "github.com"));
        assert!(!match_domain("github.com", "google.com"));
    }

    #[test]
    fn domain_match_wildcard() {
        assert!(match_domain("api.github.com", "*.github.com"));
        assert!(match_domain("github.com", "*.github.com"));
        assert!(!match_domain("github.io", "*.github.com"));
    }

    #[test]
    fn domain_case_insensitive() {
        assert!(match_domain("GitHub.com", "github.com"));
    }

    #[test]
    fn is_allowed_denies_blocked() {
        let allowed = vec!["*.github.com".into()];
        let denied = vec!["evil.github.com".into()];
        assert!(is_domain_allowed("api.github.com", &allowed, &denied));
        assert!(!is_domain_allowed("evil.github.com", &allowed, &denied));
    }

    #[test]
    fn is_allowed_empty_list_denies_all() {
        assert!(!is_domain_allowed("github.com", &[], &[]));
    }
}
