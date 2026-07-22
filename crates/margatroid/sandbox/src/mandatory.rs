//! 强制保护路径
//!
//! 这些路径在代码中硬编码为禁止写入，不受任何配置影响。

/// 无论如何都禁止写入的路径列表
///
/// 返回的路径支持 `~` 前缀（表示用户主目录）。
pub fn mandatory_deny_write() -> &'static [&'static str] {
    &[
        "~/.ssh",
        "~/.ssh/",
        "~/.aws",
        "~/.aws/",
        "~/.gnupg",
        "~/.gnupg/",
        ".env",
        ".gitconfig",
        ".git/hooks/",
        ".mcp.json",
    ]
}

/// 无论如何都禁止读取的路径列表
pub fn mandatory_deny_read() -> &'static [&'static str] {
    &["~/.ssh/id_rsa", "~/.ssh/id_ed25519", "~/.ssh/id_ecdsa"]
}

/// 展开路径中的 `~` 为用户主目录
pub fn expand_home(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().into_owned();
        }
    }
    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mandatory_lists_are_non_empty() {
        assert!(!mandatory_deny_write().is_empty());
        assert!(!mandatory_deny_read().is_empty());
    }

    #[test]
    fn expand_home_with_tilde() {
        let result = expand_home("~/.ssh/id_rsa");
        assert!(!result.starts_with('~'));
        assert!(result.ends_with("/.ssh/id_rsa"));
    }

    #[test]
    fn expand_home_without_tilde() {
        assert_eq!(expand_home("/etc/passwd"), "/etc/passwd");
    }
}
