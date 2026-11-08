//! Capture the typed Lua failure before `Display` flattens callback causes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LuaFailureClass {
    Syntax,
    Runtime,
    Callback,
    External,
    Memory,
    Other,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LuaFailureDiagnostic {
    pub class: LuaFailureClass,
    pub message: String,
    pub traceback: Option<String>,
}
impl LuaFailureDiagnostic {
    pub fn capture(error: &mlua::Error) -> Self {
        Self::capture_with(error, |_| None)
    }
    /// Integrations can extract a sanitized message from their own typed external error.
    /// This callback never receives a stringified Lua wrapper.
    pub fn capture_with(
        error: &mlua::Error,
        external_message: impl Fn(&(dyn std::error::Error + 'static)) -> Option<String>,
    ) -> Self {
        fn walk(
            error: &mlua::Error,
            message: &dyn Fn(&(dyn std::error::Error + 'static)) -> Option<String>,
        ) -> LuaFailureDiagnostic {
            use mlua::Error;
            match error {
                Error::CallbackError { traceback, cause } => {
                    let mut captured = walk(cause, message);
                    if captured.traceback.is_none() {
                        captured.traceback = Some(traceback.clone());
                    }
                    captured
                }
                Error::WithContext { context, cause } => {
                    let mut captured = walk(cause, message);
                    captured.message = format!("{context}: {}", captured.message);
                    captured
                }
                Error::ExternalError(cause) => {
                    // An external error can itself be a typed mlua error.
                    if let Some(inner) = cause.downcast_ref::<Error>() {
                        return walk(inner, message);
                    }
                    LuaFailureDiagnostic {
                        class: LuaFailureClass::External,
                        message: message(cause.as_ref()).unwrap_or_else(|| cause.to_string()),
                        traceback: None,
                    }
                }
                Error::SyntaxError { message, .. } => LuaFailureDiagnostic {
                    class: LuaFailureClass::Syntax,
                    message: strip_location(message).to_owned(),
                    traceback: None,
                },
                Error::RuntimeError(text) => {
                    let (value, trace) = text
                        .split_once("\nstack traceback:")
                        .map_or((text.as_str(), None), |(value, trace)| {
                            (value, Some(format!("stack traceback:{trace}")))
                        });
                    LuaFailureDiagnostic {
                        class: LuaFailureClass::Runtime,
                        message: strip_location(value).to_owned(),
                        traceback: trace,
                    }
                }
                Error::MemoryError(message) => LuaFailureDiagnostic {
                    class: LuaFailureClass::Memory,
                    message: message.clone(),
                    traceback: None,
                },
                _ => LuaFailureDiagnostic {
                    class: LuaFailureClass::Other,
                    message: error.to_string(),
                    traceback: None,
                },
            }
        }
        walk(error, &external_message)
    }
}
// Only remove the anchored location grammar emitted by Lua/mlua. Business
// messages, including embedded file-like strings, remain untouched.
fn strip_location(message: &str) -> &str {
    let rest = if message.starts_with("[string \"") {
        let Some(end) = message.find("\"]:") else {
            return message;
        };
        &message[end + 3..]
    } else if message.split(':').next().is_some_and(|prefix| {
        prefix.contains('/')
            || prefix.contains('\\')
            || prefix.rsplit_once('.').is_some_and(|(name, extension)| {
                !name.is_empty()
                    && !extension.is_empty()
                    && extension
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            })
    }) || (message.as_bytes().get(1) == Some(&b':')
        && message
            .as_bytes()
            .get(2)
            .is_some_and(|c| *c == b'/' || *c == b'\\'))
    {
        // File chunks use @path and render as an absolute/relative path:line.
        // Find a numeric line component, allowing spaces and Windows drive colons.
        for (offset, _) in message.match_indices(": ") {
            let prefix = &message[..offset];
            if let Some((_, line)) = prefix.rsplit_once(':')
                && !line.is_empty()
                && line.bytes().all(|c| c.is_ascii_digit())
            {
                return &message[offset + 2..];
            }
        }
        return message;
    } else {
        return message;
    };
    let Some((line, rest)) = rest.split_once(": ") else {
        return message;
    };
    if !line.is_empty() && line.bytes().all(|c| c.is_ascii_digit()) {
        rest
    } else {
        message
    }
}
