/// 对上层暴露的统一 provider 错误
/// 屏蔽各 provider 内部错误细节
#[derive(Debug)]
pub enum ProviderError {
    /// 网络层错误（连接超时、DNS 失败等）
    Network(String),

    /// API 返回了错误状态码，并携带了结构化的错误信息
    Api {
        code: i32,
        message: String,
        /// provider 原始错误元数据，透传给调用方
        metadata: Option<serde_json::Value>,
    },

    /// API 返回了错误状态码，但响应体无法解析
    ApiRaw { status: u16, body: String },

    /// 响应体反序列化失败
    Deserialize { message: String, raw: String },

    /// 流式响应中单个 chunk 解析失败
    /// 通常不应中断整个流，由调用方决定是否跳过
    StreamChunk { message: String, raw: String },

    /// 请求参数非法（在发出请求之前就可以检测到）
    InvalidRequest(String),

    /// provider 不支持请求的功能
    /// 例如某 provider 不支持 vision / tool_calls
    Unsupported(String),
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Network(msg) => write!(f, "Network error: {msg}"),
            Self::Api { code, message, .. } => write!(f, "API error {code}: {message}"),
            Self::ApiRaw { status, body } => write!(f, "API error (HTTP {status}): {body}"),
            Self::Deserialize { message, raw } => {
                write!(f, "Deserialize error: {message}; raw: {raw}")
            }
            Self::StreamChunk { message, raw } => {
                write!(f, "Stream chunk error: {message}; chunk: {raw}")
            }
            Self::InvalidRequest(msg) => write!(f, "Invalid request: {msg}"),
            Self::Unsupported(msg) => write!(f, "Unsupported: {msg}"),
        }
    }
}

impl std::error::Error for ProviderError {}
