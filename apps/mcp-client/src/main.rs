use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
    sync::{Mutex, RwLock, mpsc, oneshot},
    task::JoinHandle,
};

const MAX_FRAME: usize = 128 * 1024 * 1024;
const MAX_DISCOVERY: usize = 64 * 1024 * 1024;
const MAX_PENDING: usize = 8192;
const VERSION: &str = "2025-06-18";
type Error = Value;
type Result<T> = std::result::Result<T, Error>;
fn error(code: i64, message: impl ToString) -> Error {
    json!({"code":code,"message":message.to_string()})
}
fn io_error(e: impl ToString) -> Error {
    error(-32000, e)
}

#[derive(Deserialize)]
#[serde(tag = "transport", rename_all = "lowercase", deny_unknown_fields)]
enum Server {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
    Http {
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
    },
}
struct Http {
    client: reqwest::Client,
    url: String,
    headers: reqwest::header::HeaderMap,
    session: RwLock<Option<String>>,
    version: RwLock<Option<String>>,
}
enum Transport {
    Stdio(mpsc::Sender<Value>),
    Http(Box<Http>),
}
struct Session {
    transport: Transport,
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<Value>>>>,
    next: AtomicU64,
    closed: AtomicBool,
    ready: AtomicBool,
    stop: tokio::sync::watch::Sender<bool>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

// A bounded reader preserves UTF-8 across chunks and never allocates an unbounded line.
async fn frame<R: AsyncBufRead + Unpin>(reader: &mut R) -> std::io::Result<Option<Vec<u8>>> {
    let mut bytes = Vec::new();
    loop {
        let chunk = reader.fill_buf().await?;
        if chunk.is_empty() {
            return if bytes.is_empty() {
                Ok(None)
            } else {
                Err(std::io::Error::other("EOF inside JSON line"))
            };
        }
        let length = chunk
            .iter()
            .position(|c| *c == b'\n')
            .map(|n| n + 1)
            .unwrap_or(chunk.len());
        if bytes.len() + length > MAX_FRAME {
            return Err(std::io::Error::other("JSON line exceeds 4 MiB budget"));
        }
        let complete = chunk[length - 1] == b'\n';
        bytes.extend_from_slice(&chunk[..length]);
        reader.consume(length);
        if complete {
            return Ok(Some(bytes));
        }
    }
}
impl Session {
    async fn open(server: Server) -> Result<Arc<Self>> {
        let (stop, _) = tokio::sync::watch::channel(false);
        let mut child_io = None;
        let transport = match server {
            Server::Stdio { command, args, env } => {
                let mut cmd = Command::new(command);
                cmd.args(args)
                    .envs(env)
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::inherit())
                    .kill_on_drop(true);
                #[cfg(unix)]
                cmd.process_group(0);
                let mut child = cmd.spawn().map_err(io_error)?;
                let input = child.stdin.take().unwrap();
                let output = child.stdout.take().unwrap();
                let (tx, rx) = mpsc::channel(MAX_PENDING);
                child_io = Some((child, input, output, rx));
                Transport::Stdio(tx)
            }
            Server::Http { url, headers } => {
                let parsed = reqwest::Url::parse(&url).map_err(io_error)?;
                if !matches!(parsed.scheme(), "http" | "https") {
                    return Err(io_error("HTTP transport requires http or https URL"));
                }
                let mut map = reqwest::header::HeaderMap::new();
                for (name, value) in headers {
                    map.insert(
                        reqwest::header::HeaderName::from_bytes(name.as_bytes())
                            .map_err(io_error)?,
                        value.parse().map_err(io_error)?,
                    );
                }
                let client = reqwest::Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .build()
                    .map_err(io_error)?;
                Transport::Http(Box::new(Http {
                    client,
                    url,
                    headers: map,
                    session: RwLock::new(None),
                    version: RwLock::new(None),
                }))
            }
        };
        let session = Arc::new(Self {
            transport,
            pending: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
            closed: AtomicBool::new(false),
            ready: AtomicBool::new(false),
            stop,
            tasks: Mutex::new(Vec::new()),
        });
        if let Some((mut child, mut input, output, mut rx)) = child_io {
            let weak = Arc::downgrade(&session);
            let mut stop = session.stop.subscribe();
            let writer = tokio::spawn(async move {
                loop {
                    tokio::select! {
                        biased;
                        _ = stop.changed() => break,
                        item = rx.recv() => {
                            let Some(item) = item else { break };
                            let mut bytes = serde_json::to_vec(&item).unwrap(); bytes.push(b'\n');
                            let write = async { input.write_all(&bytes).await?; input.flush().await };
                            tokio::select! {
                                _ = stop.changed() => break,
                                result = write => if let Err(e) = result {
                                    if let Some(s) = weak.upgrade() { s.fail(io_error(format!("server stdin: {e}"))).await; }
                                    break;
                                }
                            }
                        }
                    }
                }
                drop(input);
            });
            let weak = Arc::downgrade(&session);
            let mut stop = session.stop.subscribe();
            let reader = tokio::spawn(async move {
                let mut reader = BufReader::new(output);
                loop {
                    tokio::select! {
                        biased;
                        _ = stop.changed() => break,
                        line = frame(&mut reader) => {
                            let Some(s) = weak.upgrade() else { break };
                            match line {
                                Ok(Some(line)) => match serde_json::from_slice(&line) {
                                    Ok(value) => if let Err(e) = s.incoming(value).await { s.fail(e).await; break; },
                                    Err(e) => { s.fail(io_error(format!("invalid server JSON: {e}"))).await; break; }
                                },
                                Ok(None) => { s.fail(io_error("MCP server stdout closed")).await; break; },
                                Err(e) => { s.fail(io_error(e)).await; break; }
                            }
                        }
                    }
                }
            });
            let weak = Arc::downgrade(&session);
            let mut stop = session.stop.subscribe();
            let watcher = tokio::spawn(async move {
                #[cfg(unix)]
                let group = child.id();
                tokio::select! {
                    biased;
                    _ = stop.changed() => {},
                    status = child.wait() => if let Some(s) = weak.upgrade() {
                        s.fail(io_error(format!("MCP server exited: {status:?}"))).await;
                    }
                }
                // Kill the dedicated process group too: descendants can hold pipes open.
                #[cfg(unix)]
                if let Some(pid) = group {
                    unsafe {
                        libc::kill(-(pid as i32), libc::SIGKILL);
                    }
                }
                let _ = child.start_kill();
                let _ = child.wait().await;
            });
            session.tasks.lock().await.extend([writer, reader, watcher]);
        }
        Ok(session)
    }
    async fn fail(&self, reason: Error) {
        self.closed.store(true, Ordering::Release);
        let _ = self.stop.send(true);
        for (_, tx) in self.pending.lock().await.drain() {
            let _ = tx.send(Err(reason.clone()));
        }
    }
    async fn close(&self) {
        self.fail(io_error("MCP client input closed")).await;
        for task in self.tasks.lock().await.drain(..) {
            let _ = task.await;
        }
    }
    async fn incoming(&self, value: Value) -> Result<()> {
        if value.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return Err(io_error("server message is not JSON-RPC 2.0"));
        }
        if let Some(method) = value.get("method").and_then(Value::as_str) {
            if let Some(id) = value.get("id") {
                let response = if method == "ping" {
                    json!({"jsonrpc":"2.0","id":id,"result":{}})
                } else {
                    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Client capability not supported"}})
                };
                // Only stdio routes server requests through its shared writer. HTTP SSE
                // requests are handled by the stream owner to avoid recursive futures.
                if let Transport::Stdio(tx) = &self.transport {
                    tx.try_send(response).map_err(io_error)?;
                }
            } else {
                eprintln!("MCP notification: {method}");
            }
            return Ok(());
        }
        let id = value
            .get("id")
            .and_then(Value::as_u64)
            .ok_or_else(|| io_error("server response has invalid id"))?;
        let result = match (value.get("result"), value.get("error")) {
            (Some(result), None) => Ok(result.clone()),
            (None, Some(err))
                if err.get("code").and_then(Value::as_i64).is_some()
                    && err.get("message").and_then(Value::as_str).is_some() =>
            {
                Err(err.clone())
            }
            _ => {
                return Err(io_error(
                    "server response must contain result or JSON-RPC error",
                ));
            }
        };
        if let Some(tx) = self.pending.lock().await.remove(&id) {
            let _ = tx.send(result);
        } else {
            eprintln!("ignored unknown MCP response id {id}");
        }
        Ok(())
    }
    async fn request(self: &Arc<Self>, method: &str, params: Value) -> Result<Value> {
        if self.closed.load(Ordering::Acquire) {
            return Err(io_error("MCP connection closed"));
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            if pending.len() >= MAX_PENDING {
                return Err(io_error("MCP in-flight request budget exceeded"));
            }
            if self.closed.load(Ordering::Acquire) {
                return Err(io_error("MCP connection closed"));
            }
            pending.insert(id, tx);
        }
        let value = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        match &self.transport {
            Transport::Stdio(tx) => {
                if let Err(e) = tx.try_send(value) {
                    self.pending.lock().await.remove(&id);
                    return Err(io_error(e));
                }
            }
            Transport::Http(http) => {
                let result = self.http_post(http, value, true).await;
                if let Err(e) = result {
                    self.pending.lock().await.remove(&id);
                    return Err(e);
                }
                // A completed POST without the response cannot fulfill this request.
                if self.pending.lock().await.remove(&id).is_some() {
                    return Err(io_error("HTTP stream ended without matching response"));
                }
            }
        }
        rx.await.map_err(io_error)?
    }
    async fn notify(&self, method: &str, params: Value) -> Result<()> {
        let value = json!({"jsonrpc":"2.0","method":method,"params":params});
        match &self.transport {
            Transport::Stdio(tx) => tx.try_send(value).map_err(io_error),
            Transport::Http(http) => self.http_post(http, value, false).await,
        }
    }
    async fn http_builder(&self, http: &Http, method: reqwest::Method) -> reqwest::RequestBuilder {
        let mut request = http
            .client
            .request(method, &http.url)
            .headers(http.headers.clone())
            .header("Accept", "application/json, text/event-stream");
        if let Some(session) = &*http.session.read().await {
            request = request.header("Mcp-Session-Id", session);
        }
        if let Some(version) = &*http.version.read().await {
            request = request.header("MCP-Protocol-Version", version);
        }
        request
    }
    async fn http_post(&self, http: &Http, value: Value, expects_response: bool) -> Result<()> {
        let response = self
            .http_builder(http, reqwest::Method::POST)
            .await
            .json(&value)
            .send()
            .await
            .map_err(io_error)?;
        if !response.status().is_success() {
            return Err(io_error(format!("MCP HTTP status {}", response.status())));
        }
        if let Some(session) = response.headers().get("mcp-session-id") {
            let session = session.to_str().map_err(io_error)?.to_owned();
            *http.session.write().await = Some(session);
        }
        if !expects_response {
            if response.status() != reqwest::StatusCode::ACCEPTED {
                return Err(io_error("MCP notification POST requires HTTP 202"));
            }
            return Ok(());
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned();
        if content_type.starts_with("text/event-stream") {
            self.read_sse(http, response, value.get("id").and_then(Value::as_u64))
                .await
        } else if content_type.starts_with("application/json") {
            let mut stream = response.bytes_stream();
            let mut bytes = Vec::new();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(io_error)?;
                if bytes.len() + chunk.len() > MAX_FRAME {
                    return Err(io_error("HTTP JSON response exceeds frame budget"));
                }
                bytes.extend_from_slice(&chunk);
            }
            let value = serde_json::from_slice(&bytes).map_err(io_error)?;
            self.incoming(value).await
        } else {
            Err(io_error(
                "MCP HTTP response requires application/json or text/event-stream",
            ))
        }
    }
    async fn read_sse(
        &self,
        http: &Http,
        response: reqwest::Response,
        expected: Option<u64>,
    ) -> Result<()> {
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        let mut data = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(io_error)?;
            for byte in chunk {
                bytes.push(byte);
                if bytes.len() + data.len() > MAX_FRAME {
                    return Err(io_error("SSE event exceeds frame budget"));
                }
                if byte != b'\n' {
                    continue;
                }
                let line = std::str::from_utf8(&bytes)
                    .map_err(io_error)?
                    .trim_end_matches(['\r', '\n']);
                if line.is_empty() && !data.is_empty() {
                    let value: Value = serde_json::from_slice(&data).map_err(io_error)?;
                    if value.get("method").is_some() && value.get("id").is_some() {
                        let answer = if value.get("method").and_then(Value::as_str) == Some("ping")
                        {
                            json!({"jsonrpc":"2.0","id":value["id"],"result":{}})
                        } else {
                            json!({"jsonrpc":"2.0","id":value["id"],"error":{"code":-32601,"message":"Client capability not supported"}})
                        };
                        let response = self
                            .http_builder(http, reqwest::Method::POST)
                            .await
                            .json(&answer)
                            .send()
                            .await
                            .map_err(io_error)?;
                        if response.status() != reqwest::StatusCode::ACCEPTED {
                            return Err(io_error("server request response was not accepted"));
                        }
                    } else {
                        self.incoming(value).await?;
                    }
                    data.clear();
                    if let Some(id) = expected
                        && !self.pending.lock().await.contains_key(&id)
                    {
                        return Ok(());
                    }
                } else if let Some(content) = line.strip_prefix("data:") {
                    if !data.is_empty() {
                        data.push(b'\n');
                    }
                    data.extend_from_slice(content.strip_prefix(' ').unwrap_or(content).as_bytes());
                }
                bytes.clear();
            }
        }
        if !bytes.is_empty() || !data.is_empty() {
            return Err(io_error("EOF inside SSE event"));
        }
        Ok(())
    }
    async fn initialize(self: &Arc<Self>) -> Result<Value> {
        let init = self.request("initialize", json!({"protocolVersion":VERSION,"capabilities":{},"clientInfo":{"name":"mcp-client","version":env!("CARGO_PKG_VERSION")}})).await?;
        let version = init
            .get("protocolVersion")
            .and_then(Value::as_str)
            .ok_or_else(|| io_error("initialize response missing protocolVersion"))?;
        if !["2024-11-05", "2025-03-26", VERSION, "2025-11-25"].contains(&version) {
            return Err(io_error(format!(
                "unsupported MCP protocol version {version}"
            )));
        }
        if init
            .get("capabilities")
            .and_then(|v| v.get("tools"))
            .is_none()
        {
            return Err(io_error("MCP server does not advertise tools capability"));
        }
        if let Transport::Http(http) = &self.transport {
            *http.version.write().await = Some(version.to_owned());
        }
        self.notify("notifications/initialized", json!({})).await?;
        let mut tools = Vec::new();
        let mut cursor = None;
        let mut cursors = std::collections::HashSet::new();
        loop {
            let params = cursor
                .map(|cursor| json!({"cursor":cursor}))
                .unwrap_or_else(|| json!({}));
            let page = self.request("tools/list", params).await?;
            let page_tools = page
                .get("tools")
                .and_then(Value::as_array)
                .ok_or_else(|| io_error("tools/list response missing tools array"))?;
            for tool in page_tools {
                if tool.get("name").and_then(Value::as_str).is_none()
                    || !tool.get("inputSchema").is_some_and(Value::is_object)
                {
                    return Err(io_error("invalid discovered tool definition"));
                }
                tools.push(tool.clone());
            }
            if serde_json::to_vec(&tools).map_err(io_error)?.len() > MAX_DISCOVERY {
                return Err(io_error("discovered tools exceed response budget"));
            }
            match page.get("nextCursor") {
                None | Some(Value::Null) => break,
                Some(Value::String(next)) if cursors.insert(next.clone()) => {
                    cursor = Some(next.clone())
                }
                _ => return Err(io_error("invalid or repeated tools/list cursor")),
            }
        }
        self.ready.store(true, Ordering::Release);
        Ok(json!({"tools":tools}))
    }
}

#[derive(Default)]
struct State {
    session: Option<Arc<Session>>,
    registering: bool,
}
async fn operation(state: Arc<Mutex<State>>, request: &Value) -> Result<Value> {
    match request.get("operation").and_then(Value::as_str) {
        Some("register") => {
            {
                let mut state = state.lock().await;
                if state.registering || state.session.is_some() {
                    return Err(error(
                        -32600,
                        "one server connection per process; already registered",
                    ));
                }
                state.registering = true;
            }
            let server = serde_json::from_value(
                request
                    .get("server")
                    .cloned()
                    .ok_or_else(|| error(-32602, "missing server"))?,
            )
            .map_err(|e| error(-32602, e))?;
            let session = Session::open(server).await?;
            state.lock().await.session = Some(session.clone());
            let result = session.initialize().await;
            if let Err(e) = &result {
                session.fail(e.clone()).await;
            }
            result
        }
        Some("call") => {
            let session = state
                .lock()
                .await
                .session
                .clone()
                .ok_or_else(|| io_error("server not registered"))?;
            if !session.ready.load(Ordering::Acquire) {
                return Err(io_error("server initialization not complete"));
            }
            let tool = request
                .get("tool")
                .and_then(Value::as_str)
                .ok_or_else(|| error(-32602, "missing tool string"))?;
            let arguments = request
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            if !arguments.is_object() {
                return Err(error(-32602, "arguments must be an object"));
            }
            session
                .request("tools/call", json!({"name":tool,"arguments":arguments}))
                .await
        }
        _ => Err(error(
            -32601,
            "unknown operation; expected register or call",
        )),
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let state = Arc::new(Mutex::new(State::default()));
    let (output_tx, mut output_rx) = mpsc::channel::<Value>(MAX_PENDING);
    let mut writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(value) = output_rx.recv().await {
            let mut bytes = serde_json::to_vec(&value).unwrap();
            if bytes.len() > MAX_FRAME {
                bytes = serde_json::to_vec(&json!({"id":value.get("id"),"error":io_error("response exceeds frame budget")})).unwrap();
            }
            bytes.push(b'\n');
            if let Err(e) = stdout.write_all(&bytes).await {
                eprintln!("stdout: {e}");
                break;
            }
            if let Err(e) = stdout.flush().await {
                eprintln!("stdout flush: {e}");
                break;
            }
        }
    });
    let mut input = BufReader::new(tokio::io::stdin());
    let mut tasks = tokio::task::JoinSet::new();
    let active = Arc::new(Mutex::new(std::collections::HashSet::<String>::new()));
    let mut writer_done = false;
    loop {
        tokio::select! {
            biased;
            _ = &mut writer => { writer_done = true; break; },
            Some(_) = tasks.join_next(), if !tasks.is_empty() => {},
            line = frame(&mut input) => {
                let line = match line { Ok(Some(line)) => line, Ok(None) => break, Err(e) => { eprintln!("stdin: {e}"); break; } };
                let request: Value = match serde_json::from_slice(&line) {
                    Ok(value) => value,
                    Err(e) => { if output_tx.try_send(json!({"id":null,"error":error(-32700,e)})).is_err() { break; } continue; }
                };
                let id = request.get("id").cloned().unwrap_or(Value::Null);
                if !(id.is_string() || id.is_i64() || id.is_u64()) {
                    if output_tx.try_send(json!({"id":null,"error":error(-32600,"id must be a string or integer")})).is_err() { break; }
                    continue;
                }
                let key = id.to_string();
                let mut ids = active.lock().await;
                if ids.contains(&key) { eprintln!("duplicate active request id {key}; closing input"); break; }
                if ids.len() >= MAX_PENDING {
                    if output_tx.try_send(json!({"id":id,"error":io_error("local in-flight request budget exceeded")})).is_err() { break; }
                    continue;
                }
                ids.insert(key.clone()); drop(ids);
                let state = state.clone(); let output = output_tx.clone(); let active = active.clone();
                tasks.spawn(async move {
                    let response = match operation(state, &request).await {
                        Ok(result) => json!({"id":id,"result":result}),
                        Err(err) => json!({"id":id,"error":err}),
                    };
                    if output.send(response).await.is_ok() { active.lock().await.remove(&key); }
                });
            }
        }
    }
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    if let Some(session) = state.lock().await.session.clone() {
        session.close().await;
    }
    drop(output_tx);
    if !writer_done {
        let _ = writer.await;
    }
}
