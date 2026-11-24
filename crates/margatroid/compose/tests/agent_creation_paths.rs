#[path = "unified_lua.rs"]
mod runtime_support;
use address::WorldAddressExt;
use futures_util::{SinkExt, StreamExt};
use runtime_support::{DRIVER, Harness, write};
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};
type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
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
async fn next(
    h: &mut Harness,
    socket: &mut Socket,
    seen: &mut Vec<Value>,
    matches: impl Fn(&Value) -> bool,
) -> Value {
    if let Some(value) = seen.iter().find(|v| matches(v)) {
        return value.clone();
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        h.app.tick();
        assert!(
            Instant::now() < deadline,
            "expected frame missing: {seen:?}"
        );
        tokio::select! {frame=socket.next()=>{if let Message::Text(text)=frame.expect("connection ended").unwrap(){let value:Value=serde_json::from_str(&text).unwrap();seen.push(value.clone());if matches(&value){return value;}}},_=tokio::time::sleep(Duration::from_millis(1))=>{}}
    }
}
#[tokio::test]
async fn explicit_ui_sources_are_independent_and_existing_start_reuses_configuration() {
    let mut h = Harness::open();
    let driver = h.driver("explicit-ui", DRIVER);
    for name in ["alpha", "beta"] {
        write(
            h.root.path(),
            format!("initial/{name}.toml"),
            &format!("marker='{name}'\n"),
        );
        write(h.root.path(), format!("interfaces/{name}/index.html"), name);
    }
    let endpoint = loop {
        h.app.tick();
        if let Some(addr) = h
            .app
            .world()
            .get_resource::<axum_plugin::AxumHandle>()
            .unwrap()
            .local_address()
        {
            break addr;
        }
        tokio::task::yield_now().await;
    };
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{endpoint}/ws"))
        .await
        .unwrap();
    let mut seen = Vec::new();
    send(
        &mut socket,
        "connection.register",
        "register",
        json!({"client_type":"webui","name":"explicit-ui"}),
    )
    .await;
    assert_eq!(
        next(&mut h, &mut socket, &mut seen, |v| v["id"] == "register").await["success"],
        true
    );
    for name in ["alpha", "beta"] {
        let arguments = json!({"node_name":name,"config_path":h.root.path().join(format!("initial/{name}.toml")),"ui_path":h.root.path().join(format!("interfaces/{name}"))});
        send(
            &mut socket,
            "agent.start",
            name,
            json!({"mode":"new","workspace":"/demo","tool_path":driver,"arguments":arguments}),
        )
        .await;
        let reply = next(&mut h, &mut socket, &mut seen, |v| v["id"] == name).await;
        assert_eq!(reply["success"], true, "{reply}");
    }
    next(&mut h, &mut socket, &mut seen, |v| {
        v["type"] == "agent.ui.sync"
            && v["address"] == "/demo/alpha"
            && v["ui"]["files"][0]["data"] == "YWxwaGE="
    })
    .await;
    next(&mut h, &mut socket, &mut seen, |v| {
        v["type"] == "agent.ui.sync"
            && v["address"] == "/demo/beta"
            && v["ui"]["files"][0]["data"] == "YmV0YQ=="
    })
    .await;
    for name in ["alpha", "beta"] {
        let data = h
            .root
            .path()
            .join(format!("workspaces/demo/agents/normal/{name}"));
        assert!(!data.join("ui").exists());
        assert_eq!(
            std::fs::read_to_string(data.join("setting.toml")).unwrap(),
            format!("marker='{name}'\n")
        );
    }
    write(h.root.path(), "interfaces/alpha/index.html", "changed");
    next(&mut h, &mut socket, &mut seen, |v| {
        v["type"] == "agent.ui.sync"
            && v["address"] == "/demo/alpha"
            && v["ui"]["files"][0]["data"] == "Y2hhbmdlZA=="
    })
    .await;
    let alpha = h.app.world().entity_by_address("/demo/alpha").unwrap();
    let storage = h
        .app
        .world()
        .get_component::<storage_core::AgentStorage>(alpha)
        .unwrap()
        .clone();
    storage_core::execute_storage(
        &storage,
        storage_core::StorageOperation::SetConfig {
            path: "persisted".into(),
            value: toml::Value::Boolean(true),
        },
    )
    .unwrap();
    h.stop(alpha);
    assert!(!storage.is_open());
    write(
        h.root.path(),
        "initial/alpha.toml",
        "[invalid after first initialization",
    );
    send(
        &mut socket,
        "agent.start",
        "existing",
        json!({"mode":"existing","workspace":"/demo","node_name":"alpha"}),
    )
    .await;
    let restarted = next(&mut h, &mut socket, &mut seen, |v| v["id"] == "existing").await;
    assert_eq!(restarted["success"], true, "{restarted}");
    let alpha = h.app.world().entity_by_address("/demo/alpha").unwrap();
    assert_eq!(h.config(alpha)["persisted"].as_bool(), Some(true));
    assert_eq!(h.config(alpha)["marker"].as_str(), Some("alpha"));
    socket.close(None).await.unwrap();
    compose::shutdown_app(&mut h.app).await.unwrap();
}

#[test]
fn driver_mismatch_rejects_before_touching_persistent_files() {
    let mut h = Harness::open();
    let a = h.driver("driver-a", DRIVER);
    let b = h.driver("driver-b", DRIVER);
    let agent = h.start(
        h.workspace,
        &a,
        json!({"node_name":"shared","append":"original"}),
    );
    let database = h
        .app
        .world()
        .get_component::<workspace_core::Workspace>(h.workspace)
        .unwrap()
        .database
        .clone();
    let before = database.agent_by_node("shared").unwrap();
    h.stop(agent);
    let settings = h
        .root
        .path()
        .join("workspaces/demo/agents/normal/shared/setting.toml");
    let contents = std::fs::read(&settings).unwrap();
    let rejected = h.execute(
        h.workspace,
        &b,
        json!({"node_name":"shared","alias":"attacker"}),
    );
    assert!(
        rejected.as_ref().map_or(true, |result| result.failed),
        "foreign Driver unexpectedly succeeded: {rejected:?}"
    );
    assert_eq!(database.agent_by_node("shared").unwrap(), before);
    assert_eq!(std::fs::read(settings).unwrap(), contents);
    h.close();
}
