use app_runtime_plugin::RuntimePlugin;
use compose::{ApplicationOptions, build_app};
use core_plugin::{App, Entity, World};
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
        assert!(Instant::now() < deadline, "request timed out");
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn command(
    app: &mut App,
    source: Entity,
    target: Entity,
    text: &str,
    arguments: Vec<Value>,
) -> MclValue {
    let (tx, mut rx) = oneshot::channel();
    app.world().emit_event(MclCommand {
        id: format!("domain-admission:{text}"),
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
            arguments,
        },
        cancel: CancellationToken::new(),
        reply: Arc::new(Mutex::new(Some(tx))),
    });
    drive(app, &mut rx).unwrap()
}
#[test]
fn starting_and_running_mcl_reach_real_domains_which_reject_explicit_stale_turns() {
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
        "agent-drivers/probe/admission/dev/base.lua",
        r#"
function configure()return {timeout_ms=0}end
function execute(_,context)
 local agent=margatroid.agent:create({name=context.name,config_path=context.resource_dir..'/setting.toml',ui_path=context.resource_dir..'/ui'})
 margatroid:wait(agent.address)
end
"#,
    );
    write(
        root.path(),
        "workspaces/margatroid/probe/dev/workspace.yaml",
        &format!(
            "project_root: {}\nmembers:\n- name: target\n  driver: driver:probe/admission:dev\n  home_root: {}\n",
            root.path().display(),
            root.path().display()
        ),
    );
    write(
        root.path(),
        "agent-drivers/probe/admission/dev/setting.toml",
        "",
    );
    let mut app = build_app(ApplicationOptions {
        data_root: root.path().into(),
        runner_path: std::env::current_exe().unwrap(),
    })
    .unwrap();
    let submissions = Arc::new(Mutex::new((0usize, 0usize)));
    let recorded = submissions.clone();
    app.add_system(RuntimePlugin::UPDATE, move |world: &mut World| {
        let inference = world
            .event_reader::<inference_core::InferenceRequest>()
            .into_iter()
            .count();
        let tools = world
            .event_reader::<tool_core::ToolExecuteRequest>()
            .into_iter()
            .count();
        let mut counts = recorded.lock().unwrap();
        counts.0 += inference;
        counts.1 += tools;
    });
    let (tx, mut rx) = oneshot::channel();
    app.world().emit_event(StartWorkspace {
        id: "start".into(),
        path: root
            .path()
            .join("workspaces/margatroid/probe/dev/workspace.yaml"),
        reply: Arc::new(Mutex::new(Some(tx))),
    });
    let workspace = drive(&mut app, &mut rx).unwrap();
    let agent = app.world().query_with::<agent_core::Agent>().result()[0];
    let client = app.world_mut().spawn();
    app.world_mut().insert_component(
        client,
        address::Address::new("client:probe/admission:dev").unwrap(),
    );
    assert!(matches!(
        command(
            &mut app,
            client,
            agent,
            "EFFECT model_set ?",
            vec![json!({"model":"offline"})]
        ),
        MclValue::Config(_)
    ));
    for (index, state) in [
        agent_core::AgentState::Starting,
        agent_core::AgentState::Running,
    ]
    .into_iter()
    .enumerate()
    {
        if state == agent_core::AgentState::Running {
            assert_eq!(
                command(&mut app, client, agent, "EFFECT ready", vec![]),
                MclValue::Unit
            );
        }
        let result = command(
            &mut app,
            client,
            agent,
            "EFFECT inference ? ?",
            vec![
                json!([{"type":"user","content":"stale turn"}]),
                json!("stale-turn"),
            ],
        );
        assert!(
            matches!(result,MclValue::Inference{message,stop_reason}if stop_reason=="completed"&&message["content"].is_null()&&message["tool_calls"]==json!([])),
            "domain must not submit HTTP for a stale explicit turn"
        );
        let result = command(
            &mut app,
            client,
            agent,
            "EFFECT tool_call ? ?",
            vec![
                json!([{"id":"stale-turn","tool_name":"unimported","arguments":"{}"}]),
                json!("stale-turn"),
            ],
        );
        assert!(
            matches!(result,MclValue::Array(mcl_core::MessageArray::Message(rows))if rows.len()==1&&rows[0]["failed"]==true&&rows[0]["content"]=="Tool call aborted.")
        );
        assert_eq!(
            *submissions.lock().unwrap(),
            (index + 1, index + 1),
            "MCL intercepted a domain admission decision"
        );
        let current = app
            .world()
            .get_component::<agent_core::Agent>(agent)
            .unwrap();
        assert_eq!(current.state(), state);
        assert!(current.turn().is_none());
        assert!(
            app.world()
                .get_component::<inference_core::ActiveInference>(agent)
                .unwrap()
                .current
                .is_none()
        );
        assert!(
            app.world()
                .get_component::<tool_core::ToolTasks>(agent)
                .unwrap()
                .list()
                .is_empty()
        );
    }
    let result = command(
        &mut app,
        client,
        agent,
        "EFFECT inference ?",
        vec![json!([{"type":"user","content":"connection refused must be an error message"}])],
    );
    let MclValue::Inference {
        message,
        stop_reason,
    } = result
    else {
        panic!("provider failure must return an inference message");
    };
    assert_eq!(stop_reason, "error");
    assert_eq!(message["type"], "error");
    assert!(
        message["message"]
            .as_str()
            .unwrap()
            .contains("connection refused")
    );
    assert!(message["request_id"].as_str().is_some());
    assert_eq!(
        app.world()
            .get_component::<agent_core::Agent>(agent)
            .unwrap()
            .state(),
        agent_core::AgentState::Running
    );
    let (tx, mut rx) = oneshot::channel();
    app.world().emit_event(StopWorkspace {
        id: "stop".into(),
        workspace: workspace.workspace,
        reply: Arc::new(Mutex::new(Some(tx))),
    });
    drive(&mut app, &mut rx).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(compose::shutdown_app(&mut app)).unwrap();
}
