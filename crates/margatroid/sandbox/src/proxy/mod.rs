//! 网络代理模块
//!
//! HTTP 代理处理 Web 流量过滤，SOCKS5 代理处理其余 TCP 流量。
//! 当前为骨架实现——代理服务器在 Phase 3 完整实现。

pub mod http;
pub mod socks5;
