//! Execution-scoped HTTP capability shared by embedded VMs and sandboxed workers.
use futures_util::StreamExt;
use mlua::{Function, Lua, MultiValue, Table, Value};
use reqwest::{
    Client, Method, Url,
    header::{HeaderName, HeaderValue},
};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

const REQUEST_BYTES: usize = 8 * 1024 * 1024;
const RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const HEADER_BYTES: usize = 64 * 1024;
const HEADER_COUNT: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpErrorKind {
    InvalidRequest,
    Busy,
    Closed,
    Cancelled,
    Timeout,
    Transport,
    ResponseTooLarge,
    HeadersTooLarge,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpError {
    pub kind: HttpErrorKind,
    pub message: &'static str,
}
impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}
impl std::error::Error for HttpError {}
fn error(kind: HttpErrorKind, message: &'static str) -> HttpError {
    HttpError { kind, message }
}
fn invalid(message: &'static str) -> HttpError {
    error(HttpErrorKind::InvalidRequest, message)
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpHeader {
    pub name: String,
    pub value: Vec<u8>,
}
#[derive(Clone, Debug)]
pub struct HttpRequestOptions {
    pub url: String,
    pub method: String,
    pub headers: Vec<HttpHeader>,
    pub body: Option<Vec<u8>>,
    pub timeout_ms: u64,
    pub max_response_bytes: usize,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<HttpHeader>,
    pub body: Vec<u8>,
}
struct Admission {
    accepting: bool,
    active: usize,
}
struct Shared {
    admission: Mutex<Admission>,
    idle: Notify,
}
struct RequestGuard(Arc<Shared>);
impl Drop for RequestGuard {
    fn drop(&mut self) {
        let mut admission = self.0.admission.lock().unwrap_or_else(|e| e.into_inner());
        admission.active -= 1;
        drop(admission);
        self.0.idle.notify_waiters();
    }
}
#[derive(Clone)]
pub struct HttpContext {
    client: Client,
    cancel: CancellationToken,
    deadline: Option<Instant>,
    shared: Arc<Shared>,
}
impl HttpContext {
    pub fn new(cancel: CancellationToken, deadline: Option<Instant>) -> Result<Self, HttpError> {
        let client = Client::builder()
            .use_rustls_tls()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| {
                error(
                    HttpErrorKind::Transport,
                    "HTTP client initialization failed",
                )
            })?;
        Ok(Self {
            client,
            cancel: cancel.child_token(),
            deadline,
            shared: Arc::new(Shared {
                admission: Mutex::new(Admission {
                    accepting: true,
                    active: 0,
                }),
                idle: Notify::new(),
            }),
        })
    }
    pub async fn close_and_wait(&self) {
        self.shared
            .admission
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .accepting = false;
        self.cancel.cancel();
        loop {
            let notified = self.shared.idle.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self
                .shared
                .admission
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .active
                == 0
            {
                break;
            }
            notified.await;
        }
    }
    pub async fn request(&self, options: HttpRequestOptions) -> Result<HttpResponse, HttpError> {
        let (method, url, headers) = validate(&options)?;
        let _guard = {
            let mut admission = self
                .shared
                .admission
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if !admission.accepting {
                return Err(error(HttpErrorKind::Closed, "HTTP execution is closed"));
            }
            if admission.active >= 8 {
                return Err(error(HttpErrorKind::Busy, "HTTP concurrency limit reached"));
            }
            admission.active += 1;
            RequestGuard(self.shared.clone())
        };
        let deadline = Instant::now()
            .checked_add(Duration::from_millis(options.timeout_ms))
            .ok_or_else(|| invalid("invalid timeout"))?;
        let deadline = self.deadline.map_or(deadline, |outer| outer.min(deadline));
        let operation = async {
            let mut request = self.client.request(method, url).headers(headers);
            if !request_headers_accept_encoding(&options.headers) {
                request = request.header(reqwest::header::ACCEPT_ENCODING, "identity");
            }
            if let Some(body) = options.body {
                request = request.body(body);
            }
            let response = request.send().await.map_err(transport_error)?;
            let status = response.status().as_u16();
            let mut count = 0usize;
            let mut size = 0usize;
            let mut headers = Vec::new();
            for (name, value) in response.headers() {
                count += 1;
                size = size
                    .saturating_add(name.as_str().len())
                    .saturating_add(value.as_bytes().len());
                if count > HEADER_COUNT || size > HEADER_BYTES {
                    return Err(error(
                        HttpErrorKind::HeadersTooLarge,
                        "HTTP response headers exceed limit",
                    ));
                }
                headers.push(HttpHeader {
                    name: name.as_str().to_owned(),
                    value: value.as_bytes().to_vec(),
                });
            }
            let mut body = Vec::new();
            if options.method != "HEAD" {
                if response
                    .content_length()
                    .is_some_and(|length| length > options.max_response_bytes as u64)
                {
                    return Err(error(
                        HttpErrorKind::ResponseTooLarge,
                        "HTTP response body exceeds limit",
                    ));
                }
                let mut stream = response.bytes_stream();
                while let Some(bytes) = stream.next().await {
                    let bytes = bytes.map_err(transport_error)?;
                    if bytes.len() > options.max_response_bytes.saturating_sub(body.len()) {
                        return Err(error(
                            HttpErrorKind::ResponseTooLarge,
                            "HTTP response body exceeds limit",
                        ));
                    }
                    body.extend_from_slice(&bytes);
                }
            }
            Ok(HttpResponse {
                status,
                headers,
                body,
            })
        };
        tokio::select! {biased;
            _=self.cancel.cancelled()=>Err(error(HttpErrorKind::Cancelled,"HTTP request cancelled")),
            result=tokio::time::timeout_at(tokio::time::Instant::from_std(deadline),operation)=>result.unwrap_or_else(|_|Err(error(HttpErrorKind::Timeout,"HTTP request timed out"))),
        }
    }
}
fn transport_error(error_value: reqwest::Error) -> HttpError {
    if error_value.is_timeout() {
        error(HttpErrorKind::Timeout, "HTTP request timed out")
    } else {
        error(HttpErrorKind::Transport, "HTTP transport failed")
    }
}
fn request_headers_accept_encoding(headers: &[HttpHeader]) -> bool {
    headers
        .iter()
        .any(|header| header.name.eq_ignore_ascii_case("accept-encoding"))
}
fn validate(
    options: &HttpRequestOptions,
) -> Result<(Method, Url, reqwest::header::HeaderMap), HttpError> {
    if !(1..=120000).contains(&options.timeout_ms)
        || !(1..=RESPONSE_BYTES).contains(&options.max_response_bytes)
    {
        return Err(invalid("invalid HTTP request limits"));
    }
    let method = match options.method.as_str() {
        "GET" => Method::GET,
        "HEAD" => Method::HEAD,
        "POST" => Method::POST,
        "PUT" => Method::PUT,
        "PATCH" => Method::PATCH,
        "DELETE" => Method::DELETE,
        "OPTIONS" => Method::OPTIONS,
        _ => return Err(invalid("unsupported HTTP method")),
    };
    if options
        .body
        .as_ref()
        .is_some_and(|body| body.len() > REQUEST_BYTES)
    {
        return Err(invalid("HTTP request body exceeds limit"));
    }
    if (method == Method::GET || method == Method::HEAD) && options.body.is_some() {
        return Err(invalid("GET and HEAD cannot carry a body"));
    }
    let url = Url::parse(&options.url).map_err(|_| invalid("invalid HTTP URL"))?;
    let authority = options
        .url
        .split_once("://")
        .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or_default());
    if authority.is_some_and(|value| value.contains('@')) {
        return Err(invalid("HTTP URL cannot contain user information"));
    }
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid("invalid HTTP URL"));
    }
    let mut headers = reqwest::header::HeaderMap::new();
    let mut size = 0usize;
    if options.headers.len() > HEADER_COUNT {
        return Err(error(
            HttpErrorKind::HeadersTooLarge,
            "HTTP request headers exceed limit",
        ));
    }
    for header in &options.headers {
        size = size
            .saturating_add(header.name.len())
            .saturating_add(header.value.len());
        if size > HEADER_BYTES {
            return Err(error(
                HttpErrorKind::HeadersTooLarge,
                "HTTP request headers exceed limit",
            ));
        }
        let name = HeaderName::from_bytes(header.name.as_bytes())
            .map_err(|_| invalid("invalid HTTP header name"))?;
        if matches!(
            name.as_str(),
            "host"
                | "content-length"
                | "transfer-encoding"
                | "connection"
                | "upgrade"
                | "proxy-authorization"
                | "proxy-connection"
                | "keep-alive"
                | "te"
                | "trailer"
        ) {
            return Err(invalid("HTTP transport header cannot be overridden"));
        }
        let value = HeaderValue::from_bytes(&header.value)
            .map_err(|_| invalid("invalid HTTP header value"))?;
        headers.append(name, value);
    }
    Ok((method, url, headers))
}
fn parse(options: Table) -> Result<HttpRequestOptions, HttpError> {
    for pair in options.clone().pairs::<Value, Value>() {
        let (key, _) = pair.map_err(|_| invalid("invalid HTTP options"))?;
        let Value::String(key) = key else {
            return Err(invalid("HTTP options require named fields"));
        };
        if !matches!(
            key.to_str()
                .map_err(|_| invalid("invalid HTTP option name"))?
                .as_ref(),
            "url" | "method" | "headers" | "body" | "timeout_ms" | "max_response_bytes"
        ) {
            return Err(invalid("unknown HTTP option"));
        }
    }
    fn text(value: Value) -> Result<String, HttpError> {
        match value {
            Value::String(value) => value
                .to_str()
                .map(|s| s.to_string())
                .map_err(|_| invalid("HTTP option must be UTF-8")),
            _ => Err(invalid("HTTP option must be a string")),
        }
    }
    fn integer(value: Value, default: u64) -> Result<u64, HttpError> {
        match value {
            Value::Nil => Ok(default),
            Value::Integer(value) if value > 0 => Ok(value as u64),
            _ => Err(invalid("HTTP limit must be a positive integer")),
        }
    }
    let get = |key| {
        options
            .raw_get::<Value>(key)
            .map_err(|_| invalid("invalid HTTP options"))
    };
    let url = text(get("url")?)?;
    let method = match get("method")? {
        Value::Nil => "GET".to_owned(),
        value => text(value)?,
    };
    let body = match get("body")? {
        Value::Nil => None,
        Value::String(value) => Some(value.as_bytes().to_vec()),
        _ => return Err(invalid("HTTP body must be a byte string")),
    };
    let timeout_ms = integer(get("timeout_ms")?, 30000)?;
    let max_response_bytes =
        usize::try_from(integer(get("max_response_bytes")?, REQUEST_BYTES as u64)?)
            .map_err(|_| invalid("invalid HTTP response limit"))?;
    let mut headers = Vec::new();
    match get("headers")? {
        Value::Nil => {}
        Value::Table(table) => {
            let length = table.raw_len();
            let mut count = 0usize;
            for pair in table.clone().pairs::<Value, Value>() {
                let (key, _) = pair.map_err(|_| invalid("invalid HTTP headers"))?;
                if !matches!(key,Value::Integer(index) if index>0 && (index as usize)<=length) {
                    return Err(invalid("HTTP headers must be a continuous array"));
                }
                count += 1;
            }
            if count != length {
                return Err(invalid("HTTP headers must be a continuous array"));
            }
            if length > HEADER_COUNT {
                return Err(error(
                    HttpErrorKind::HeadersTooLarge,
                    "HTTP request headers exceed limit",
                ));
            }
            for index in 1..=length {
                let entry = table
                    .raw_get::<Table>(index)
                    .map_err(|_| invalid("invalid HTTP header"))?;
                for pair in entry.clone().pairs::<Value, Value>() {
                    let (key, _) = pair.map_err(|_| invalid("invalid HTTP header"))?;
                    if !matches!(key,Value::String(key) if key.as_bytes().as_ref()==b"name" || key.as_bytes().as_ref()==b"value")
                    {
                        return Err(invalid("unknown HTTP header field"));
                    }
                }
                let name = text(
                    entry
                        .raw_get::<Value>("name")
                        .map_err(|_| invalid("invalid HTTP header"))?,
                )?;
                let Value::String(value) = entry
                    .raw_get::<Value>("value")
                    .map_err(|_| invalid("invalid HTTP header"))?
                else {
                    return Err(invalid("HTTP header value must be a byte string"));
                };
                headers.push(HttpHeader {
                    name,
                    value: value.as_bytes().to_vec(),
                });
            }
        }
        _ => return Err(invalid("HTTP headers must be an array")),
    }
    Ok(HttpRequestOptions {
        url,
        method,
        headers,
        body,
        timeout_ms,
        max_response_bytes,
    })
}
pub fn install_http(lua: &Lua, context: HttpContext) -> mlua::Result<Function> {
    lua.create_async_function(move |lua, args: MultiValue| {
        let context = context.clone();
        async move {
            if args.len() != 1 {
                return Err(mlua::Error::external(invalid(
                    "HTTP request expects one options object",
                )));
            }
            let Some(Value::Table(options)) = args.front() else {
                return Err(mlua::Error::external(invalid(
                    "HTTP request expects an options object",
                )));
            };
            let options = parse(options.clone()).map_err(mlua::Error::external)?;
            let response = context
                .request(options)
                .await
                .map_err(mlua::Error::external)?;
            let value = lua.create_table()?;
            value.raw_set("status", response.status)?;
            value.raw_set("body", lua.create_string(response.body)?)?;
            let headers = lua.create_table()?;
            for (index, header) in response.headers.into_iter().enumerate() {
                let entry = lua.create_table()?;
                entry.raw_set("name", header.name)?;
                entry.raw_set("value", lua.create_string(header.value)?)?;
                headers.raw_set(index + 1, entry)?;
            }
            value.raw_set("headers", headers)?;
            Ok(value)
        }
    })
}
