/// 对敏感字符串脱敏，只保留前后各 4 个字符
pub fn redact(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();

    if len <= 8 {
        return "*".repeat(len);
    }

    let prefix: String = chars[..4].iter().collect();
    let suffix: String = chars[len - 4..].iter().collect();
    format!("{}****{}", prefix, suffix)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redact_long() {
        let result = redact("sk-ant-1234567890abcdef");
        assert!(result.starts_with("sk-a"));
        assert!(result.ends_with("cdef"));
        assert!(result.contains("****"));
    }

    #[test]
    fn test_redact_short() {
        let result = redact("abc");
        assert_eq!(result, "***");
    }
}
