use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    time::timeout,
};

const LIMIT: Duration = Duration::from_secs(10);
fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/server.py")
}
struct Host {
    child: Child,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
}
impl Host {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_mcp-client"))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let input = child.stdin.take();
        let output = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            input,
            output,
        }
    }
    async fn send(&mut self, value: Value) {
        let mut bytes = serde_json::to_vec(&value).unwrap();
        bytes.push(b'\n');
        timeout(LIMIT, self.input.as_mut().unwrap().write_all(&bytes))
            .await
            .unwrap()
            .unwrap();
    }
    async fn receive(&mut self) -> Value {
        let mut line = String::new();
        assert!(
            timeout(LIMIT, self.output.read_line(&mut line))
                .await
                .expect("response timed out")
                .unwrap()
                > 0,
            "unexpected host EOF"
        );
        serde_json::from_str(&line).expect("stdout must contain only JSON response frames")
    }
    async fn register(&mut self, server: Value) {
        self.send(json!({"id":"register","operation":"register","server":server}))
            .await;
        let response = self.receive().await;
        assert_eq!(response["id"], "register");
        assert_eq!(
            response["result"]["tools"].as_array().unwrap().len(),
            2,
            "{response}"
        );
        assert_eq!(response["result"]["tools"][0]["description"], "回声");
    }
    async fn close(&mut self) {
        drop(self.input.take());
        assert!(
            timeout(LIMIT, self.child.wait())
                .await
                .expect("EOF cleanup timed out")
                .unwrap()
                .success()
        );
    }
}
fn stdio() -> Value {
    json!({"transport":"stdio","command":"python3","args":[fixture()],"env":{"MCP_FIXTURE_ENV":"present"}})
}

#[tokio::test]
async fn stdio_discovery_concurrent_out_of_order_errors_and_eof_cleanup() {
    let mut host = Host::start();
    host.register(stdio()).await;
    host.send(json!({"id":1,"operation":"call","tool":"echo","arguments":{"reverse":true,"text":"first"}})).await;
    host.send(json!({"id":"second","operation":"call","tool":"echo","arguments":{"reverse":true,"text":"第二"}})).await;
    let second = host.receive().await;
    let first = host.receive().await;
    assert_eq!(second["id"], "second");
    assert_eq!(second["result"]["content"][0]["text"], "第二");
    assert_eq!(first["id"], 1);
    host.send(json!({"id":3,"operation":"call","tool":"echo","arguments":{"error":true}}))
        .await;
    let error = host.receive().await;
    assert_eq!(
        error["error"],
        json!({"code":-32602,"message":"fixture failure","data":{"detail":42}})
    );
    host.send(json!({"id":4,"operation":"call","tool":"echo","arguments":{"isError":true}}))
        .await;
    assert_eq!(host.receive().await["result"]["isError"], true);
    host.send(json!({"id":5,"operation":"call","tool":"echo","arguments":{"pid":true}}))
        .await;
    let response = host.receive().await;
    let pids: Value =
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(pids["env"], "present");
    // Pending work must not delay genuine stdin EOF or leave server descendants running.
    host.send(json!({"id":6,"operation":"call","tool":"echo","arguments":{"hang":true}}))
        .await;
    host.close().await;
    #[cfg(target_os = "linux")]
    for key in ["pid", "child"] {
        let status = std::fs::read_to_string(format!("/proc/{}/stat", pids[key].as_u64().unwrap()));
        if let Ok(status) = status {
            assert_eq!(
                status.split(") ").nth(1).unwrap().chars().next(),
                Some('Z'),
                "{key} is still running: {status}"
            );
        }
    }
}

#[tokio::test]
async fn large_catalog_preserves_all_definitions_and_routes_boundary_tools() {
    let mut host = Host::start();
    let mut server = stdio();
    server["env"]["MCP_FIXTURE_LARGE_CATALOG"] = json!("1");
    host.send(json!({"id":"large-register","operation":"register","server":server}))
        .await;
    let response = host.receive().await;
    assert_eq!(response["id"], "large-register");
    assert!(response.get("error").is_none(), "{response}");
    let tools = response["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1024);
    let catalog_bytes = serde_json::to_vec(tools).unwrap().len();
    assert!((8 * 1024 * 1024..=12 * 1024 * 1024).contains(&catalog_bytes));
    for (index, tool) in tools.iter().enumerate() {
        let marker = format!("{index:04}");
        assert_eq!(tool["name"], format!("catalog_{marker}"));
        assert_eq!(
            tool["description"],
            format!(
                "catalog-description-{marker}:{}:description-tail-{marker}",
                "x".repeat(8192)
            )
        );
        assert_eq!(
            tool["inputSchema"],
            json!({
                "type":"object", "title":format!("catalog-tool-{marker}"),
                "properties": {
                    "text":{"type":"string","description":format!("schema-property-tail-{marker}")},
                    "reverse":{"type":"boolean"}, "pid":{"type":"boolean"},
                    "hang":{"type":"boolean"},
                },
                "additionalProperties":true, "$comment":format!("catalog-schema-tail-{marker}"),
            })
        );
    }
    host.send(json!({"id":"large-first","operation":"call","tool":"catalog_0000","arguments":{"reverse":true,"text":"first"}})).await;
    host.send(json!({"id":"large-last","operation":"call","tool":"catalog_1023","arguments":{"reverse":true,"text":"last"}})).await;
    let last = host.receive().await;
    let first = host.receive().await;
    assert_eq!(last["id"], "large-last");
    assert_eq!(last["result"]["content"][0]["text"], "last");
    assert_eq!(first["id"], "large-first");
    assert_eq!(first["result"]["content"][0]["text"], "first");
    host.close().await;
    println!(
        "large_catalog protocol: tools=1024 catalog_json_bytes={catalog_bytes}; all definitions and reverse boundary calls passed"
    );
}

#[tokio::test]
async fn remote_exit_fails_every_pending_request() {
    let mut host = Host::start();
    host.register(stdio()).await;
    host.send(json!({"id":1,"operation":"call","tool":"echo","arguments":{"hang":true}}))
        .await;
    host.send(json!({"id":2,"operation":"call","tool":"echo","arguments":{"exit":true}}))
        .await;
    let a = host.receive().await;
    let b = host.receive().await;
    assert!(a.get("error").is_some(), "{a}");
    assert!(b.get("error").is_some(), "{b}");
    assert_ne!(a["id"], b["id"]);
    host.close().await;
}

#[tokio::test]
async fn http_json_sse_headers_and_concurrent_out_of_order() {
    let mut server = Command::new("python3")
        .arg(fixture())
        .arg("http")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut port = String::new();
    timeout(
        LIMIT,
        BufReader::new(server.stdout.take().unwrap()).read_line(&mut port),
    )
    .await
    .unwrap()
    .unwrap();
    let mut host = Host::start();
    host.register(json!({"transport":"http","url":format!("http://127.0.0.1:{}/mcp",port.trim()),"headers":{"X-Fixture":"test"}})).await;
    host.send(
        json!({"id":"a","operation":"call","tool":"echo","arguments":{"reverse":true,"text":"a"}}),
    )
    .await;
    host.send(
        json!({"id":"b","operation":"call","tool":"echo","arguments":{"reverse":true,"text":"b"}}),
    )
    .await;
    let second = host.receive().await;
    let first = host.receive().await;
    assert_eq!(second["id"], "b");
    assert_eq!(first["id"], "a");
    host.send(
        json!({"id":3,"operation":"call","tool":"echo","arguments":{"json":true,"error":true}}),
    )
    .await;
    assert_eq!(host.receive().await["error"]["data"]["detail"], 42);
    host.send(json!({"id":4,"operation":"call","tool":"echo","arguments":{"json":true,"text":"JSON response"}})).await;
    assert_eq!(
        host.receive().await["result"]["content"][0]["text"],
        "JSON response"
    );
    host.close().await;
    server.kill().await.unwrap();
    server.wait().await.unwrap();
}

#[tokio::test]
async fn malformed_inputs_and_failed_registration_are_explicit_errors() {
    let mut host = Host::start();
    host.input
        .as_mut()
        .unwrap()
        .write_all(b"not json\n")
        .await
        .unwrap();
    assert_eq!(host.receive().await["error"]["code"], -32700);
    host.send(json!({"id":1,"operation":"call","tool":"echo"}))
        .await;
    assert!(host.receive().await.get("error").is_some());
    host.send(json!({"id":2,"operation":"register","server":{"transport":"stdio","command":"/no/such/mcp-fixture-command"}})).await;
    assert!(host.receive().await.get("error").is_some());
    host.close().await;
}
