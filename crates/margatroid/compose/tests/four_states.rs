use app_runtime_plugin::RuntimePlugin;
use client_core::{ClientFrame, ClientIdentity, ClientSender, Clients, ConnectionId};
use compose::{ApplicationOptions, build_app};
use core_plugin::{App, Entity};
use mcl_core::{MclCommand, MclInstruction, MclValue};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use workspace_core::{StartWorkspace, StopWorkspace};

fn write(root: &Path, name: &str, text: &str) {
    let path = root.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}
fn drive<T>(app: &mut App, rx: &mut oneshot::Receiver<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        app.tick();
        if let Ok(value) = rx.try_recv() {
            return value;
        }
        assert!(Instant::now() < deadline, "lifecycle timed out");
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn command(app: &mut App, source: Entity, target: Entity, text: &str) -> MclValue {
    let (tx, mut rx) = oneshot::channel();
    app.world().emit_event(MclCommand {
        id: format!("check:{text}"),
        source,
        source_address: app
            .world()
            .get_component::<address::Address>(source)
            .unwrap()
            .as_str()
            .into(),
        source_instance: None,
        target,
        instruction: MclInstruction {
            text: text.into(),
            arguments: vec![],
        },
        cancel: CancellationToken::new(),
        reply: Arc::new(Mutex::new(Some(tx))),
    });
    drive(app, &mut rx).unwrap()
}
#[test]
fn automatic_lua_failure_persists_and_pushes_logs_and_error_without_ready_or_refresh() {
    let root = tempfile::tempdir().unwrap();
    write(
        root.path(),
        "config.toml",
        "[server]\nbind='127.0.0.1:0'\n[outbound]\nlogs=['type:none']\nbackend_state=['type:webui']\n",
    );
    write(
        root.path(),
        "models.toml",
        "[[models]]\nid='offline'\nmodel='unused'\nprovider='openai'\napi_type='chat_completions'\nbase_url='http://127.0.0.1:1/v1'\napi_key='unused'\ncontext_window='128k'\n",
    );
    for directory in ["tools", "prompts", "sandboxes", "skills"] {
        std::fs::create_dir_all(root.path().join(directory)).unwrap();
    }
    write(
        root.path(),
        "agent-drivers/probe/failure/latest/base.lua",
        r#"
function configure()return {timeout_ms=0}end
function execute(_,context)
 if context.name=='control' then margatroid.task:return_result('ready');margatroid.task:receive();return end
 local agent=margatroid.agent:create({owner=context.workspace.address,name=context.name,config_path=context.resource_dir..'/setting.toml',ui_path=context.resource_dir..'/ui'})
 margatroid:log('info','starting-probe')
 margatroid:wait(agent.address)
 local text='EFFECT model_set ?'
 local result=margatroid.mcl:execute(agent.address,text,{})
 if result.Error then error(string.format('%s [MCL: %s; request_id: %s]',result.Error.message,text,result.Error.request_id),2) end
 margatroid.mcl:execute(agent.address,'EFFECT ready')
end
"#,
    );
    write(
        root.path(),
        "workspaces/probe/workspace.yaml",
        &format!(
            "name: probe\nproject_root: {}\nmembers:\n- name: broken\n  driver: driver:probe/failure:latest\n  home_root: {}\n- name: control\n  driver: driver:probe/failure:latest\n  home_root: {}\n",
            root.path().display(),
            root.path().display(),
            root.path().display()
        ),
    );
    write(
        root.path(),
        "agent-drivers/probe/failure/latest/setting.toml",
        "",
    );
    let mut app = build_app(ApplicationOptions {
        data_root: root.path().into(),
        runner_path: std::env::current_exe().unwrap(),
    })
    .unwrap();
    let clients = app.world().get_resource::<Clients>().unwrap().clone();
    let (sender, mut frames) = ClientSender::channel(256);
    let connection = ConnectionId::new(901);
    clients.adopt(connection, sender).unwrap();
    let client = app.world_mut().spawn();
    app.world_mut().insert_component(
        client,
        address::Address::new("client:webui/state-probe").unwrap(),
    );
    clients
        .commit_registration(
            connection,
            Arc::new(ClientIdentity {
                client,
                address: "client:webui/state-probe".into(),
                client_type: "webui".into(),
                name: "state-probe".into(),
            }),
        )
        .unwrap();
    let (tx, mut rx) = oneshot::channel();
    app.world().emit_event(StartWorkspace {
        id: "start".into(),
        path: root.path().join("workspaces/probe/workspace.yaml"),
        reply: Arc::new(Mutex::new(Some(tx))),
    });
    let workspace = drive(&mut app, &mut rx).unwrap();
    assert!(workspace.member_errors.is_empty());
    let agent = app.world().query_with::<agent_core::Agent>().result()[0];
    assert_eq!(
        app.world()
            .get_component::<agent_core::Agent>(agent)
            .unwrap()
            .state(),
        agent_core::AgentState::Starting
    );
    assert!(matches!(
        command(&mut app, client, agent, "CONFIG GET"),
        MclValue::Config(_)
    ));
    assert!(matches!(
        command(&mut app, client, agent, "EFFECT start_turn"),
        MclValue::Error(_)
    ));
    command(&mut app, client, agent, "EFFECT wake");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        app.tick();
        if app
            .world()
            .get_component::<agent_core::Agent>(agent)
            .unwrap()
            .state()
            == agent_core::AgentState::Error
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "VM monitor did not publish error"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let storage = app
        .world()
        .get_component::<storage_core::AgentStorage>(agent)
        .unwrap();
    let logs = storage_core::read_agent_logs(storage, None, 50).unwrap();
    assert_eq!(logs.len(), 2);
    assert_eq!(logs[0].record.message, "starting-probe");
    assert!(
        logs[1]
            .record
            .raw_error
            .as_deref()
            .unwrap()
            .contains("EFFECT model_set ?")
    );
    assert!(
        logs[1]
            .record
            .raw_error
            .as_deref()
            .unwrap()
            .contains("request_id:")
    );
    assert!(
        logs[1]
            .record
            .traceback
            .as_deref()
            .is_some_and(|trace| trace.contains("base.lua"))
    );
    assert!(matches!(
        command(&mut app, client, agent, "CONFIG GET"),
        MclValue::Error(_)
    ));
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let received = rt.block_on(async {
        let mut messages = Vec::<Value>::new();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let frame = frames.recv().await.unwrap();
                if let ClientFrame::Text(text) = frame {
                    messages.push(serde_json::from_str(&text).unwrap());
                }
                if messages.iter().any(|value| {
                    value["type"] == "agent.log.sync"
                        && value["records"].as_array().is_some_and(|rows| {
                            rows.iter().any(|row| {
                                row["raw_error"]
                                    .as_str()
                                    .is_some_and(|raw| raw.contains("EFFECT model_set ?"))
                            })
                        })
                }) && messages.iter().any(|value| {
                    value["type"] == "agent.sync" && value["agents"][0]["status"] == "error"
                }) {
                    break;
                }
            }
        })
        .await
        .unwrap();
        messages
    });
    assert!(
        received
            .iter()
            .filter(|value| value["type"] == "agent.sync")
            .all(|value| value["agents"][0]["status"] != "running"),
        "Agent was falsely made ready"
    );
    let execution = app
        .world()
        .query_with::<lua_runtime_core::WorkspaceLuaTask>()
        .result()
        .into_iter()
        .find(|entity| {
            app.world()
                .get_component::<lua_runtime_core::WorkspaceLuaTask>(*entity)
                .unwrap()
                .name
                == "control"
        })
        .unwrap();
    let vm = app
        .world()
        .get_component::<lua_runtime_core::DriverLuaVm>(execution)
        .unwrap();
    let identity = lua_runtime_core::LuaExecutionIdentity {
        source: execution,
        workspace: workspace.workspace,
        address: app
            .world()
            .get_component::<address::Address>(execution)
            .unwrap()
            .as_str()
            .into(),
        instance: vm.instance.start_id.clone(),
        cancel: vm.cancel.clone(),
    };
    let services = app
        .world()
        .get_resource::<lua_runtime_core::LuaServices>()
        .unwrap()
        .clone();
    let target = app
        .world()
        .get_component::<address::Address>(agent)
        .unwrap()
        .as_str()
        .to_owned();
    let (tx, mut accepted) = oneshot::channel();
    async_runtime_plugin::WorldAsyncExt::send_async_closure(
        app.world(),
        RuntimePlugin::UPDATE,
        move || async move {
            let _ = tx.send(
                services
                    .call(identity, "agent_stop".into(), json!(target))
                    .await,
            );
            Ok::<(), async_runtime_plugin::AsyncTaskError>(())
        },
    );
    assert_eq!(drive(&mut app, &mut accepted).unwrap(), json!({}));
    let deadline = Instant::now() + Duration::from_secs(10);
    while app.world().is_alive(agent) {
        app.tick();
        assert!(
            Instant::now() < deadline,
            "injected stop could not clean an error Agent"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let (tx, mut rx) = oneshot::channel();
    app.world().emit_event(StopWorkspace {
        id: "stop".into(),
        workspace: workspace.workspace,
        reply: Arc::new(Mutex::new(Some(tx))),
    });
    drive(&mut app, &mut rx).unwrap();
    assert!(!app.world().is_alive(agent));
    rt.block_on(compose::shutdown_app(&mut app)).unwrap();
}
