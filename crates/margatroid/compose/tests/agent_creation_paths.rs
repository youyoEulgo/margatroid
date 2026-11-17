use compose::{ApplicationOptions, build_app};
use core_plugin::App;
use futures_util::{SinkExt, StreamExt};
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
    app: &mut App,
    socket: &mut Socket,
    seen: &mut Vec<Value>,
    matches: impl Fn(&Value) -> bool,
) -> Value {
    if let Some(value) = seen.iter().find(|value| matches(value)) {
        return value.clone();
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        app.tick();
        assert!(
            Instant::now() < deadline,
            "expected UI update did not arrive: {seen:?}"
        );
        tokio::select! {frame=socket.next()=>{let frame=frame.expect("connection closed").unwrap();if let Message::Text(text)=frame{let value:Value=serde_json::from_str(&text).unwrap();seen.push(value.clone());if matches(&value){return value;}}},_=tokio::time::sleep(Duration::from_millis(1))=>{}}
    }
}
#[tokio::test]
async fn lua_creation_persists_only_configuration_and_each_agent_monitors_its_explicit_ui_directory()
 {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), "config.toml", "[server]\nbind='127.0.0.1:0'\n");
    write(
        root.path(),
        "models.toml",
        "[[models]]\nid='offline'\nmodel='unused'\nprovider='openai'\napi_type='chat_completions'\nbase_url='http://127.0.0.1:1/v1'\napi_key='unused'\ncontext_window='128k'\n",
    );
    for name in ["tools", "prompts", "sandboxes", "skills"] {
        std::fs::create_dir_all(root.path().join(name)).unwrap();
    }
    write(
        root.path(),
        "agent-drivers/probe/create/latest/base.lua",
        r#"
function configure()return {timeout_ms=0}end
function execute(_,context)
 local agent=margatroid.agent:create({owner=context.workspace.address,name=context.name,config_path=context.workspace.project_root..'/initial/'..context.name..'.toml',ui_path=context.workspace.project_root..'/interfaces/'..context.name})
 local result=margatroid.mcl:execute(agent.address,'EFFECT ready');assert(not result.Error)
 margatroid.task:return_result(agent.address)
 margatroid.task:receive()
end
"#,
    );
    write(
        root.path(),
        "agent-drivers/probe/create/latest/setting.toml",
        "[invalid unrelated config must not be read by the Lua loader",
    );
    for name in ["alpha", "beta"] {
        write(
            root.path(),
            &format!("initial/{name}.toml"),
            &format!("marker='{name}'\n"),
        );
        write(root.path(), &format!("interfaces/{name}/index.html"), name);
    }
    write(
        root.path(),
        "workspaces/probe/workspace.yaml",
        &format!(
            "name: probe\nproject_root: {}\nmembers:\n- name: alpha\n  driver: driver:probe/create:latest\n  home_root: {}\n- name: beta\n  driver: driver:probe/create:latest\n  home_root: {}\n",
            root.path().display(),
            root.path().display(),
            root.path().display()
        ),
    );
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
        json!({"client_type":"webui","name":"explicit-ui"}),
    )
    .await;
    next(&mut app, &mut socket, &mut seen, |value| {
        value["id"] == "register"
    })
    .await;
    send(
        &mut socket,
        "workspace.start",
        "start",
        json!({"path":root.path().join("workspaces/probe/workspace.yaml")}),
    )
    .await;
    let started = next(&mut app, &mut socket, &mut seen, |value| {
        value["id"] == "start"
    })
    .await;
    assert_eq!(started["success"], true, "{started}");
    let alpha = "agent:probe/alpha:latest";
    let beta = "agent:probe/beta:latest";
    next(&mut app, &mut socket, &mut seen, |value| {
        value["type"] == "agent.ui.sync"
            && value["address"] == alpha
            && value["ui"]["files"][0]["data"] == "YWxwaGE="
    })
    .await;
    next(&mut app, &mut socket, &mut seen, |value| {
        value["type"] == "agent.ui.sync"
            && value["address"] == beta
            && value["ui"]["files"][0]["data"] == "YmV0YQ=="
    })
    .await;
    for name in ["alpha", "beta"] {
        let home = root.path().join("workspaces/probe/agents").join(name);
        assert!(
            !home.join("ui").exists(),
            "UI must not be copied into Agent data"
        );
        assert_eq!(
            std::fs::read_to_string(home.join("setting.toml")).unwrap(),
            format!("marker='{name}'\n")
        );
    }
    write(
        root.path(),
        "workspaces/probe/agents/alpha/ui/index.html",
        "legacy-copy-must-be-ignored",
    );
    let beta_updates = seen
        .iter()
        .filter(|value| value["type"] == "agent.ui.sync" && value["address"] == beta)
        .count();
    write(root.path(), "interfaces/alpha/index.html", "changed");
    next(&mut app, &mut socket, &mut seen, |value| {
        value["type"] == "agent.ui.sync"
            && value["address"] == alpha
            && value["ui"]["files"][0]["data"] == "Y2hhbmdlZA=="
    })
    .await;
    assert_eq!(
        seen.iter()
            .filter(|value| value["type"] == "agent.ui.sync" && value["address"] == beta)
            .count(),
        beta_updates,
        "an unrelated Agent must not receive another UI directory's update"
    );
    assert_eq!(
        std::fs::read_to_string(
            root.path()
                .join("workspaces/probe/agents/alpha/ui/index.html")
        )
        .unwrap(),
        "legacy-copy-must-be-ignored"
    );
    send(
        &mut socket,
        "mcl.execute",
        "persist",
        json!({"address":alpha,"command":"CONFIG SET ? ?","arguments":["persisted",true]}),
    )
    .await;
    assert_eq!(
        next(&mut app, &mut socket, &mut seen, |value| value["id"]
            == "persist")
        .await["success"],
        true
    );
    write(
        root.path(),
        "initial/alpha.toml",
        "[invalid initial config after first initialization",
    );
    send(
        &mut socket,
        "agent.restart",
        "restart",
        json!({"address":alpha}),
    )
    .await;
    let restarted = next(&mut app, &mut socket, &mut seen, |value| {
        value["id"] == "restart"
    })
    .await;
    assert_eq!(restarted["success"], true, "{restarted}");
    let settings = std::fs::read_to_string(
        root.path()
            .join("workspaces/probe/agents/alpha/setting.toml"),
    )
    .unwrap();
    assert!(settings.contains("alpha") && settings.contains("persisted = true"));
    let entity = app
        .world()
        .query_with::<agent_core::Agent>()
        .result()
        .into_iter()
        .find(|entity| {
            app.world()
                .get_component::<address::Address>(*entity)
                .is_some_and(|address| address.as_str() == alpha)
        })
        .unwrap();
    let resources = app
        .world()
        .get_component::<agent_core::AgentResources>(entity)
        .unwrap();
    assert_eq!(
        resources.parameters.ui_path,
        root.path().join("interfaces/alpha")
    );
    socket.close(None).await.unwrap();
    compose::shutdown_app(&mut app).await.unwrap();
}
