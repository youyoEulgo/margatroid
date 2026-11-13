use agent_core::{Agent, AgentStartParameters, AgentState, RestartAgent, StartAgent, StopAgent};
use agent_plugin::AgentPlugin;
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin};
use core_plugin::{App, Entity, World};
use inference_core::{ActiveInference, InitInference, ModelRoute};
use lua_runtime_core::{DriverLuaVm, LuaRuntimeConfig};
use lua_runtime_plugin::LuaRuntimePlugin;
use mcl_core::{MclCommand, MclInstruction, MclQueue};
use mcl_plugin::MclPlugin;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    process::Command,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use storage_core::{
    AgentStorage, MessageSelection, MessageSelector, MessageSource, StorageOperation, StorageValue,
    execute_storage,
};
use storage_plugin::StoragePlugin;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use tool_core::{InitToolTasks, ToolTasks};
use workspace_core::{WorkspaceExecutor, WorkspaceState, spawn_workspace_executor};

const PROGRAM: &str = r#"
margatroid.driver:mcl("CREATE BLOCK conversation (messages)")
margatroid.driver:mcl("CREATE TEMP BLOCK private (messages)")
local previous = margatroid.driver:mcl("GET private.messages").Array.Message
if #previous ~= 0 then error("TEMP messages restored from a previous VM") end
margatroid.driver:mcl("INJECT ? TO private.messages", {{{type="user", content="this VM only"}}})
margatroid.driver:mcl("EFFECT ready")
margatroid.driver:wait()
"#;
fn seed(root: &Path) {
    let driver = root.join("agent-drivers/local/test/latest");
    std::fs::create_dir_all(&driver).unwrap();
    std::fs::write(
        driver.join("driver.toml"),
        "schema_version=1\n[inference]\nmodel='test-model'\n",
    )
    .unwrap();
    std::fs::write(driver.join("base.lua"), PROGRAM).unwrap();
    std::fs::write(driver.join("setting.toml"), "").unwrap();
    for directory in [
        root.join("workspaces/demo"),
        root.join("project"),
        root.join("home"),
    ] {
        std::fs::create_dir_all(directory).unwrap();
    }
}
fn initialize_domains(world: &mut World) {
    let tools: Vec<_> = world
        .event_reader::<InitToolTasks>()
        .into_iter()
        .cloned()
        .collect();
    for request in tools {
        if !world.contains_component::<ToolTasks>(request.agent) {
            world.insert_component(request.agent, test_tasks());
        }
        if let Some(reply) = request.reply.lock().unwrap().take() {
            let _ = reply.send(Ok(()));
        }
    }
    let inference: Vec<_> = world
        .event_reader::<InitInference>()
        .into_iter()
        .cloned()
        .collect();
    for request in inference {
        if !world.contains_component::<ActiveInference>(request.agent) {
            world.insert_component(request.agent, ActiveInference::default());
        }
        if let Some(reply) = request.reply.lock().unwrap().take() {
            let _ = reply.send(Ok(()));
        }
    }
}
struct Fixture {
    app: App,
    workspace: Entity,
    parameters: AgentStartParameters,
}
impl Fixture {
    fn open(root: &Path) -> Self {
        let mut app = App::new();
        app.world_mut()
            .insert_resource(tool_resource_core::ToolResourceSyncBinding::new(|_| {
                client_core::UpdateProbe::new(|_| {})
            }));
        app.world_mut()
            .insert_resource(sandbox_resource_core::SandboxSyncBinding::new(|_| {
                client_core::UpdateProbe::new(|_| {})
            }));
        app.world_mut()
            .insert_resource(sandbox_resource_core::SandboxSelectionSyncBinding::new(
                |_| client_core::UpdateProbe::new(|_| {}),
            ));
        app.world_mut()
            .insert_resource(inference_core::ModelSyncBinding::new(|_| {
                client_core::UpdateProbe::new(|_| {})
            }));

        app.add_plugin(RuntimePlugin::default())
            .add_plugin(ClosurePlugin)
            .add_closure_system(RuntimePlugin::UPDATE)
            .add_plugin(AsyncRuntimePlugin)
            .add_plugin(StoragePlugin::new())
            .add_plugin(
                LuaRuntimePlugin::new()
                    .with_config(LuaRuntimeConfig {
                        startup_timeout: Some(Duration::from_secs(2)),
                        ..LuaRuntimeConfig::default()
                    })
                    .unwrap(),
            )
            .add_plugin(MclPlugin::new())
            .add_plugin(AgentPlugin::new(test_agent_context()).with_resource_roots(root))
            .add_system(RuntimePlugin::UPDATE, initialize_domains);
        app.world_mut()
            .insert_resource(inference_core::ModelRoutes::new(BTreeMap::from([(
                "test-model".into(),
                ModelRoute {
                    model: "test".into(),
                    provider: "test".into(),
                    api_type: "chat".into(),
                    base_url: "https://unused.invalid".into(),
                    api_key: String::new(),
                    context_window_tokens: 4096,
                    thinking: None,
                    reasoning_effort: None,
                },
            )])));
        let workspace = app.world_mut().spawn();
        let directory = root.join("workspaces/demo");
        app.world_mut().insert_component(
            workspace,
            workspace_core::Workspace::new(
                "demo".into(),
                directory.join("workspace.yaml"),
                root.join("project"),
                WorkspaceState::Running,
                BTreeSet::new(),
                None,
                crate::test_workspace_context(),
            ),
        );
        app.world_mut()
            .insert_component(workspace, spawn_workspace_executor().unwrap());
        Self {
            app,
            workspace,
            parameters: AgentStartParameters {
                driver: "driver:local/test:latest".into(),
                project_root: root.join("project"),
                home_root: root.join("home"),
                database_path: directory.join("agents/coder/state.sqlite"),
                settings_path: directory.join("agents/coder/setting.toml"),
            },
        }
    }
    fn start(&mut self) -> Entity {
        let (sender, mut receiver) = oneshot::channel();
        self.app.world().emit_event(StartAgent {
            id: "start".into(),
            workspace: self.workspace,
            name: "coder".into(),
            parameters: self.parameters.clone(),
            reply: Arc::new(Mutex::new(Some(sender))),
        });
        wait(&mut self.app, &mut receiver).unwrap()
    }
    fn stop(&mut self, agent: Entity) {
        let (sender, mut receiver) = oneshot::channel();
        self.app.world().emit_event(StopAgent {
            id: "stop".into(),
            agent,
            reply: Arc::new(Mutex::new(Some(sender))),
        });
        assert_eq!(wait(&mut self.app, &mut receiver).unwrap(), agent);
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        for owner in self.app.world().query_with::<DriverLuaVm>().result() {
            let vm = self
                .app
                .world()
                .get_component::<DriverLuaVm>(owner)
                .unwrap();
            vm.cancel.cancel();
            vm.wake.lock().unwrap().close();
        }
        for owner in self.app.world().query_with::<MclQueue>().result() {
            let queue = self
                .app
                .world_mut()
                .get_component_mut::<MclQueue>(owner)
                .unwrap();
            queue.accepting = false;
            queue.cancel.cancel();
        }
        if let Some(executor) = self
            .app
            .world()
            .get_component::<WorkspaceExecutor>(self.workspace)
        {
            let close = executor.close.clone();
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(close.close_and_join())
                .unwrap();
        }
    }
}
fn wait<T>(app: &mut App, receiver: &mut oneshot::Receiver<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        app.tick();
        match receiver.try_recv() {
            Ok(value) => return value,
            Err(oneshot::error::TryRecvError::Closed) => panic!("real lifecycle reply closed"),
            Err(_) => {}
        }
        assert!(Instant::now() < deadline, "real lifecycle timed out");
        std::thread::yield_now();
    }
}
fn advance_until(app: &mut App, condition: impl Fn(&World) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !condition(app.world()) {
        app.tick();
        assert!(Instant::now() < deadline, "VM transition timed out");
        std::thread::yield_now();
    }
}
fn mcl(
    app: &App,
    source: Entity,
    target: Entity,
    source_instance: Option<String>,
    id: &str,
) -> oneshot::Receiver<Result<mcl_core::MclValue, mcl_core::MclError>> {
    let (sender, receiver) = oneshot::channel();
    app.world().emit_event(MclCommand {
        id: id.into(),
        source,
        source_address: app
            .world()
            .get_component::<address::Address>(source)
            .unwrap()
            .as_str()
            .to_owned(),
        target,
        source_instance,
        instruction: MclInstruction {
            text: "CONFIG GET".into(),
            arguments: vec![],
        },
        cancel: CancellationToken::new(),
        reply: Arc::new(Mutex::new(Some(sender))),
    });
    receiver
}
#[test]
fn real_agent_restart_allows_current_driver_initialization_but_refuses_external_and_stale_sources()
{
    let root = tempfile::tempdir().unwrap();
    seed(root.path());
    let mut fixture = Fixture::open(root.path());
    let agent = fixture.start();
    let old = fixture
        .app
        .world()
        .get_component::<DriverLuaVm>(agent)
        .unwrap()
        .instance
        .clone();
    let storage = fixture
        .app
        .world()
        .get_component::<AgentStorage>(agent)
        .unwrap()
        .clone();
    execute_storage(
        &storage,
        StorageOperation::ReplaceMessages {
            sources: vec![MessageSource::Literal(vec![
                serde_json::json!({"type":"user","content":"persistent"}),
            ])],
            target: MessageSelector {
                block: "conversation".into(),
                field: "messages".into(),
                instance: None,
                selection: MessageSelection::All,
            },
        },
    )
    .unwrap();
    let (sender, mut restarted) = oneshot::channel();
    fixture.app.world().emit_event(RestartAgent {
        id: "restart".into(),
        workspace: fixture.workspace,
        name: "coder".into(),
        parameters: fixture.parameters.clone(),
        reply: Arc::new(Mutex::new(Some(sender))),
    });
    advance_until(&mut fixture.app, |world| {
        world
            .get_component::<Agent>(agent)
            .is_some_and(|agent| agent.state() == AgentState::Restarting)
            && world
                .get_component::<DriverLuaVm>(agent)
                .is_some_and(|vm| vm.instance != old)
    });
    let external = fixture.app.world_mut().spawn();
    fixture.app.world_mut().insert_component(
        external,
        address::Address::new("client:local/external:latest").unwrap(),
    );
    let mut external_reply = mcl(
        &fixture.app,
        external,
        agent,
        None,
        "external-during-restart",
    );
    let mut stale_reply = mcl(
        &fixture.app,
        agent,
        agent,
        Some(old.start_id.clone()),
        "old-driver-during-restart",
    );
    let mut unbound_reply = mcl(
        &fixture.app,
        agent,
        agent,
        None,
        "unbound-self-during-restart",
    );
    for reply in [&mut external_reply, &mut stale_reply, &mut unbound_reply] {
        let mcl_core::MclValue::Error(message) = wait(&mut fixture.app, reply).unwrap() else {
            panic!("error message")
        };
        assert_eq!(message["type"], "error");
        assert!(
            message["message"]
                .as_str()
                .unwrap()
                .starts_with("SourceInvalid:")
        );
    }
    assert_eq!(wait(&mut fixture.app, &mut restarted).unwrap(), agent);
    assert_eq!(
        fixture
            .app
            .world()
            .get_component::<Agent>(agent)
            .unwrap()
            .state(),
        AgentState::Idle
    );
    let new = fixture
        .app
        .world()
        .get_component::<DriverLuaVm>(agent)
        .unwrap()
        .instance
        .clone();
    assert_ne!(old.start_id, new.start_id);
    assert_eq!(
        execute_storage(
            fixture
                .app
                .world()
                .get_component::<AgentStorage>(agent)
                .unwrap(),
            StorageOperation::ReadMessages {
                selectors: vec![MessageSelector {
                    block: "conversation".into(),
                    field: "messages".into(),
                    instance: Some(new.start_id),
                    selection: MessageSelection::All
                }]
            }
        )
        .unwrap(),
        StorageValue::Messages(vec![
            serde_json::json!({"type":"user","content":"persistent"})
        ])
    );
    fixture.stop(agent);
}
#[test]
fn vm_instances_and_temp_namespaces_are_fresh_across_process_restarts() {
    const ROOT: &str = "MARGATROID_REAL_VM_PROBE_ROOT";
    const OUTPUT: &str = "MARGATROID_REAL_VM_PROBE_OUTPUT";
    if let Some(root) = std::env::var_os(ROOT) {
        let mut fixture = Fixture::open(Path::new(&root));
        let agent = fixture.start();
        let id = fixture
            .app
            .world()
            .get_component::<DriverLuaVm>(agent)
            .unwrap()
            .instance
            .start_id
            .clone();
        std::fs::write(std::env::var_os(OUTPUT).unwrap(), id).unwrap();
        fixture.stop(agent);
        return;
    }
    let root = tempfile::tempdir().unwrap();
    seed(root.path());
    let mut ids = Vec::new();
    for index in 0..2 {
        let output_path = root.path().join(format!("instance-{index}.txt"));
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "vm_instances_and_temp_namespaces_are_fresh_across_process_restarts",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(ROOT, root.path())
            .env(OUTPUT, &output_path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "VM process {index} failed: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        ids.push(std::fs::read_to_string(output_path).unwrap());
    }
    assert_ne!(ids[0], ids[1]);
    assert!(ids.iter().all(|id| id.starts_with("driver-")));
}

fn test_binding() -> tool_core::TaskSyncBinding {
    tool_core::TaskSyncBinding::new(
        client_core::Clients::default(),
        std::sync::Arc::from([]),
        |snapshot| {
            serde_json::to_string(&snapshot)
                .map(|text| std::sync::Arc::new(client_core::ClientFrame::Text(text)))
                .map_err(|_| client_core::ClientPrepareError::EncodeFailed)
        },
    )
}
fn test_tasks() -> tool_core::ToolTasks {
    let mut app = core_plugin::App::new();
    let owner = app.world_mut().spawn();
    tool_core::ToolTasks::new(owner, "agent:test/unit:latest".into(), test_binding())
}

fn test_agent_context() -> agent_core::AgentUpdateContext {
    agent_core::AgentUpdateContext::new(|_, _, _| Vec::new(), |_, _| {})
}

fn test_workspace_context() -> workspace_core::WorkspaceUpdateContext {
    workspace_core::WorkspaceUpdateContext::new(|_, _| {})
}
