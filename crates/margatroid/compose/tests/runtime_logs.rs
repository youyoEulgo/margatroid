use compose::{ApplicationOptions, build_app};
use core_plugin::App;
use futures_util::{SinkExt, StreamExt};
use log_plugin::WorldEventLogExt;
use serde_json::{Value, json};
use std::{
    path::Path,
    time::{Duration, Instant},
};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};
type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
fn write(root: &Path, name: &str, text: &str) {
    let path = root.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}
async fn next(
    app: &mut App,
    socket: &mut Socket,
    seen: &mut Vec<Value>,
    matches: impl Fn(&Value) -> bool,
) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        app.tick();
        assert!(
            Instant::now() < deadline,
            "persistent log frame did not arrive"
        );
        tokio::select! {frame=socket.next()=>{let frame=frame.expect("connection closed").unwrap();if let Message::Text(text)=frame{let value:Value=serde_json::from_str(&text).unwrap();seen.push(value.clone());if matches(&value){return value;}}},_=tokio::time::sleep(Duration::from_millis(1))=>{}}
    }
}
async fn send(socket: &mut Socket, kind: &str, id: &str, message: Value) {
    socket
        .send(Message::Text(
            json!({"type":kind,"id":id,"message":message})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
}
#[tokio::test]
async fn frontend_and_runtime_reports_commit_before_sync_and_retry_does_not_duplicate() {
    let root = tempfile::tempdir().unwrap();
    write(
        root.path(),
        "config.toml",
        "[server]\nbind='127.0.0.1:0'\n[outbound]\nlogs=['type:webui']\nbackend_state=['type:none']\n",
    );
    write(
        root.path(),
        "models.toml",
        "[[models]]\nid='offline'\nmodel='unused'\nprovider='openai'\napi_type='chat_completions'\nbase_url='http://127.0.0.1:1/v1'\napi_key='unused'\ncontext_window='128k'\n",
    );
    for name in [
        "workspaces",
        "agent-drivers",
        "tools",
        "prompts",
        "sandboxes",
        "skills",
    ] {
        std::fs::create_dir_all(root.path().join(name)).unwrap();
    }
    let mut app = build_app(ApplicationOptions {
        data_root: root.path().into(),
        runner_path: std::env::current_exe().unwrap(),
    })
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let address = loop {
        app.tick();
        if let Some(address) = app
            .world()
            .get_resource::<axum_plugin::AxumHandle>()
            .unwrap()
            .local_address()
        {
            break address;
        }
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(1)).await;
    };
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
        .await
        .unwrap();
    let mut seen = Vec::new();
    send(
        &mut socket,
        "connection.register",
        "register",
        json!({"client_type":"webui","name":"diagnostic-test"}),
    )
    .await;
    next(&mut app, &mut socket, &mut seen, |value| {
        value["id"] == "register"
    })
    .await;
    let report = json!({"report_id":"client-report-1","created_at_ms":1234,"level":"error","target":"ui.decode","agent":null,"message":"decoder rejected a frame","details_json":{"raw_frame":"{unsupported","code":"InvalidFrame"},"context_json":{"route":"activity"},"origin":"workbench.ts:812","raw_error":"SyntaxError: unexpected token","traceback":"SyntaxError: unexpected token\n at decode (workbench.ts:812)"});
    send(&mut socket, "log.append", "append-1", report.clone()).await;
    let ack = next(&mut app, &mut socket, &mut seen, |value| {
        value["id"] == "append-1"
    })
    .await;
    assert_eq!(ack["success"], true, "{ack}");
    let sequence = ack["sequence"].as_u64().unwrap();
    if !seen.iter().any(|value| {
        value["type"] == "log.sync" && value["records"][0]["report_id"] == "client-report-1"
    }) {
        next(&mut app, &mut socket, &mut seen, |value| {
            value["type"] == "log.sync" && value["records"][0]["report_id"] == "client-report-1"
        })
        .await;
    }
    let store = app
        .world()
        .get_resource::<storage_core::RuntimeLogStore>()
        .unwrap()
        .clone();
    let saved = store
        .read(storage_core::RecordRange {
            after_sequence: None,
            before_sequence: None,
            limit: 100,
        })
        .unwrap()
        .records
        .into_iter()
        .find(|row| row.sequence == sequence)
        .unwrap();
    assert_eq!(
        saved.input.record.raw_error.as_deref(),
        report["raw_error"].as_str()
    );
    assert_eq!(
        saved.input.record.traceback.as_deref(),
        report["traceback"].as_str()
    );
    assert_eq!(
        saved.input.record.details_json.as_ref().unwrap(),
        &report["details_json"]
    );
    assert!(
        saved.input.record.context_json.as_ref().unwrap()["reporting_client"]["address"]
            .as_str()
            .unwrap()
            .starts_with("client:webui/")
    );
    send(&mut socket, "log.append", "append-retry", report).await;
    let ack = next(&mut app, &mut socket, &mut seen, |value| {
        value["id"] == "append-retry"
    })
    .await;
    assert_eq!(ack["sequence"], sequence);
    assert_eq!(
        seen.iter()
            .filter(|value| value["type"] == "log.sync"
                && value["records"][0]["report_id"] == "client-report-1")
            .count(),
        1
    );
    app.world()
        .event_log(log_plugin::LogLevel::Error, "durable runtime fault");
    let runtime = next(&mut app, &mut socket, &mut seen, |value| {
        value["type"] == "log.sync" && value["records"][0]["message"] == "durable runtime fault"
    })
    .await;
    assert!(
        runtime["records"][0]["traceback"]
            .as_str()
            .is_some_and(|trace| !trace.is_empty())
    );
    assert!(runtime["records"][0]["origin"].as_str().is_some());
    send(
        &mut socket,
        "agent.ui.req",
        "ui-error",
        json!({"address":"agent:missing/target"}),
    )
    .await;
    let failure = next(&mut app, &mut socket, &mut seen, |value| {
        value["id"] == "ui-error"
    })
    .await;
    assert_eq!(failure["success"], false);
    let reports = store
        .read(storage_core::RecordRange {
            after_sequence: None,
            before_sequence: None,
            limit: 100,
        })
        .unwrap();
    assert!(reports.records.iter().any(|row| {
        row.input.target == "agent.ui.req"
            && row
                .input
                .record
                .details_json
                .as_ref()
                .is_some_and(|details| details["response"]["id"] == "ui-error")
    }));
    send(&mut socket, "log.req", "history", json!({"limit":100})).await;
    let page = next(&mut app, &mut socket, &mut seen, |value| {
        value["id"] == "history"
    })
    .await;
    assert_eq!(page["success"], true, "{page}");
    assert!(
        page["records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["report_id"] == "client-report-1" && row["sequence"] == sequence)
    );
    socket.close(None).await.unwrap();
    compose::shutdown_app(&mut app).await.unwrap();
    let reopened = storage_core::open_runtime_logs(
        &root.path().join("runtime.sqlite"),
        storage_core::RuntimeLogProbe::new(|_| {}),
    )
    .unwrap();
    assert!(
        reopened
            .read(storage_core::RecordRange {
                after_sequence: None,
                before_sequence: None,
                limit: 100
            })
            .unwrap()
            .records
            .iter()
            .any(|row| row.sequence == sequence)
    );
}
