//! Work secret 解码与 URL 构建

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use types::bridge::{BridgeError, WorkSecret};

/// 验证 ID 只含安全字符
pub fn validate_bridge_id(id: &str, label: &str) -> Result<(), BridgeError> {
    if id
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        Ok(())
    } else {
        Err(BridgeError::InvalidId {
            label: label.to_string(),
        })
    }
}

/// 解码 base64url 编码的 work secret
pub fn decode_work_secret(secret: &str) -> Result<WorkSecret, BridgeError> {
    let json = URL_SAFE_NO_PAD
        .decode(secret)
        .map_err(|e| BridgeError::WorkSecret(format!("base64 decode failed: {e}")))?;

    let ws: WorkSecret = serde_json::from_slice(&json)
        .map_err(|e| BridgeError::WorkSecret(format!("JSON parse failed: {e}")))?;

    if ws.version != 1 {
        return Err(BridgeError::WorkSecret(format!(
            "unsupported work secret version: {}",
            ws.version
        )));
    }

    if ws.session_ingress_token.is_empty() {
        return Err(BridgeError::WorkSecret(
            "missing or empty session_ingress_token".into(),
        ));
    }

    Ok(ws)
}

/// 构建 v1 Session-Ingress WebSocket URL
pub fn build_sdk_url(api_base_url: &str, session_id: &str) -> String {
    let is_local = api_base_url.contains("localhost") || api_base_url.contains("127.0.0.1");
    let protocol = if is_local { "ws" } else { "wss" };
    let version = if is_local { "v2" } else { "v1" };
    let host = api_base_url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/');
    format!("{protocol}://{host}/{version}/session_ingress/ws/{session_id}")
}

/// 构建 CCR v2 session URL（HTTP/S）
pub fn build_ccr_v2_sdk_url(api_base_url: &str, session_id: &str) -> String {
    let base = api_base_url.trim_end_matches('/');
    format!("{base}/v1/code/sessions/{session_id}")
}

/// 比较两个 session ID 是否指向同一个 session（忽略前缀差异）
pub fn same_session_id(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    // 独立函数代替闭包，避免生命周期推断问题
    fn body(s: &str) -> &str {
        s.rsplit('_').next().unwrap_or(s)
    }
    let a_body = body(a);
    let b_body = body(b);
    a_body.len() >= 4 && a_body == b_body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_sdk_url_prod() {
        let url = build_sdk_url("https://api.example.com", "sess_123");
        assert_eq!(url, "wss://api.example.com/v1/session_ingress/ws/sess_123");
    }

    #[test]
    fn test_build_sdk_url_local() {
        let url = build_sdk_url("http://localhost:8080", "sess_123");
        assert_eq!(url, "ws://localhost:8080/v2/session_ingress/ws/sess_123");
    }

    #[test]
    fn test_same_session_id() {
        assert!(same_session_id("session_abc123", "cse_abc123"));
        assert!(same_session_id("foo", "foo"));
        assert!(!same_session_id("session_abc", "session_xyz"));
    }

    #[test]
    fn test_validate_id_ok() {
        assert!(validate_bridge_id("abc-123_DEF", "test").is_ok());
    }

    #[test]
    fn test_validate_id_fail() {
        assert!(validate_bridge_id("abc/def", "test").is_err());
    }
}
