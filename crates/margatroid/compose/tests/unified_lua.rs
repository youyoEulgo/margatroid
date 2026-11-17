use compose::{ApplicationOptions, build_app};
use core_plugin::{App, Entity};
use mcl_core::{MclCommand, MclInstruction, MclValue};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use workspace_core::{StartWorkspace, StopWorkspace};

const PROGRAM: &str = r#"
function configure()return {timeout_ms=0}end
function execute(_,context)
 local child=context.arguments and context.arguments.child
 if not child then assert(context.agent_info==nil) end
 local owner=child and context.agent_info.address or context.workspace.address
 local agent=margatroid.agent:create({owner=owner,name=child and 'child' or context.name,config_path=context.resource_dir..'/setting.toml',ui_path=context.resource_dir..'/ui'})
 local function command(text,args)
  local result=margatroid.mcl:execute(agent.address,text,args)
  if type(result)=='table' and result.Error then error(result.Error.message) end
  return result
 end
 local empty=command('CREATE BLOCK saved (messages)')
 assert(type(empty)=='table' and empty.Error==nil and next(empty)==nil)
 assert(margatroid.data:encode('json',empty)=='{}')
 local failure=margatroid.mcl:execute(agent.address,'INVALID_MCL_INSTRUCTION')
 assert(type(failure)=='table' and failure.Error.type=='error')
 assert(type(failure.Error.message)=='string' and type(failure.Error.request_id)=='string')
 local again=command('CREATE BLOCK saved (messages)')
 assert(type(again)=='table' and margatroid.data:encode('json',again)=='{}')
 command('CREATE TEMP BLOCK transient (messages)')
 command('INJECT ? TO saved.messages',{{{type='user',content=child and 'child-data' or 'parent-data'}}})
 command('INJECT ? TO transient.messages',{{{type='user',content='temporary'}}})
 local model=command('EFFECT model_set ?',{{model='offline'}}).Config
 assert(model.context_window_tokens==128000)
 command('EFFECT ready')
 if not child then
  margatroid.tools:register({agent=agent.address,tools={{alias='automatic_registration',description='lifetime of automatic task',input_schema={type='object'},lifetime={task_id=context.task_id}}}})
 end
 if child then margatroid.task:return_result(agent.address);margatroid.task:receive();return end
 command('IMPORT TOOL tool:probe/shared:latest AS shared')
 command('EFFECT start_turn')
 local result=command('EFFECT tool_call ?',{{{id='child-create',tool_name='shared',arguments='{"child":true}'}}})
 command('CONFIG SET ? ?',{'child_address',result.Array.Message[1].content})
 command('EFFECT finish_turn')
 pcall(function()margatroid:wait(agent.address)end)
 -- The Agent going away does not own this execution. Keep running until Workspace cancellation.
 margatroid.task:return_result('agent-closed')
 margatroid.task:receive()
end
"#;
fn write(root: &Path, name: &str, text: &str) {
    let path = root.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}
fn drive<T>(app: &mut App, rx: &mut oneshot::Receiver<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        app.tick();
        if let Ok(value) = rx.try_recv() {
            return value;
        }
        assert!(Instant::now() < deadline, "operation did not settle");
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn until(app: &mut App, predicate: impl Fn(&core_plugin::World) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !predicate(app.world()) {
        app.tick();
        assert!(Instant::now() < deadline, "lifecycle did not settle");
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn command(app: &mut App, source: Entity, target: Entity, text: &str) -> MclValue {
    let (tx, mut rx) = oneshot::channel();
    app.world().emit_event(MclCommand {
        id: format!("test:{text}"),
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
fn messages(
    app: &App,
    agent: Entity,
    block: &str,
    instance: Option<String>,
) -> Result<storage_core::StorageValue, storage_core::StorageError> {
    storage_core::execute_storage(
        app.world()
            .get_component::<storage_core::AgentStorage>(agent)
            .unwrap(),
        storage_core::StorageOperation::ReadMessages {
            selectors: vec![storage_core::MessageSelector {
                block: block.into(),
                field: "messages".into(),
                instance,
                selection: storage_core::MessageSelection::All,
            }],
        },
    )
}
fn identical_lua_programs_create_independent_agents_and_stop_without_owning_the_vm() {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), "config.toml", "[server]\nbind='127.0.0.1:0'\n");
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
        "agent-drivers/probe/shared/latest/base.lua",
        PROGRAM,
    );
    write(
        root.path(),
        "agent-drivers/probe/shared/latest/setting.toml",
        "label='parent-template'\n",
    );
    write(
        root.path(),
        "agent-drivers/probe/shared/latest/ui/index.html",
        "<main>parent-ui</main>",
    );
    write(root.path(), "tools/probe/shared/latest/main.lua", PROGRAM);
    write(
        root.path(),
        "tools/probe/shared/latest/tool.toml",
        "schema_version=1\ndescription='shared Lua environment'\n",
    );
    write(
        root.path(),
        "tools/probe/shared/latest/input.schema.json",
        "{\"type\":\"object\"}",
    );
    write(
        root.path(),
        "tools/probe/shared/latest/setting.toml",
        "label='child-template'\n",
    );
    write(
        root.path(),
        "tools/probe/shared/latest/ui/index.html",
        "<main>child-ui</main>",
    );
    let definition = root.path().join("workspaces/test/workspace.yaml");
    write(
        root.path(),
        "workspaces/test/workspace.yaml",
        &format!(
            "name: test\nproject_root: {}\nmembers:\n- name: parent\n  driver: driver:probe/shared:latest\n  home_root: {}\n",
            root.path().display(),
            root.path().display()
        ),
    );
    let mut app = build_app(ApplicationOptions {
        data_root: root.path().into(),
        runner_path: std::env::current_exe().unwrap(),
    })
    .unwrap();
    let (tx, mut rx) = oneshot::channel();
    app.world().emit_event(StartWorkspace {
        id: "start".into(),
        path: definition.clone(),
        reply: Arc::new(Mutex::new(Some(tx))),
    });
    let started = drive(&mut app, &mut rx).unwrap();
    assert!(
        started.member_errors.is_empty(),
        "{:?}",
        started.member_errors
    );
    until(&mut app, |world| {
        world.query_with::<agent_core::Agent>().result().len() == 2
    });
    let agents = app.world().query_with::<agent_core::Agent>().result();
    let parent = *agents
        .iter()
        .find(|e| {
            app.world()
                .get_component::<agent_core::Agent>(**e)
                .unwrap()
                .name()
                == "parent"
        })
        .unwrap();
    let child = *agents
        .iter()
        .find(|e| {
            app.world()
                .get_component::<agent_core::Agent>(**e)
                .unwrap()
                .name()
                == "child"
        })
        .unwrap();
    assert_eq!(
        app.world()
            .get_component::<agent_core::AgentOwner>(child)
            .unwrap()
            .0,
        parent
    );
    let execution = app
        .world()
        .query_with::<lua_runtime_core::WorkspaceLuaTask>()
        .result()[0];
    assert!(
        app.world()
            .get_component::<lua_runtime_core::DriverLuaVm>(parent)
            .is_none()
    );
    assert!(
        app.world()
            .get_component::<lua_runtime_core::DriverLuaVm>(child)
            .is_none()
    );
    assert!(
        matches!(messages(&app,parent,"saved",None).unwrap(),storage_core::StorageValue::Messages(rows)if rows[0]["content"]=="parent-data")
    );
    assert!(
        matches!(messages(&app,child,"saved",None).unwrap(),storage_core::StorageValue::Messages(rows)if rows[0]["content"]=="child-data")
    );
    assert!(
        messages(
            &app,
            child,
            "transient",
            Some(
                app.world()
                    .get_component::<agent_core::AgentSession>(parent)
                    .unwrap()
                    .0
                    .clone()
            )
        )
        .is_err()
    );
    let parent_home = root.path().join("workspaces/test/agents/parent");
    let child_home = parent_home.join("agents/child");
    assert!(parent_home.join("state.sqlite").is_file());
    assert!(child_home.join("state.sqlite").is_file());
    assert!(!parent_home.join("ui").exists());
    assert!(!child_home.join("ui").exists());
    let parent_ui = app
        .world()
        .get_component::<agent_core::AgentResources>(parent)
        .unwrap()
        .parameters
        .ui_path
        .clone();
    let child_ui = app
        .world()
        .get_component::<agent_core::AgentResources>(child)
        .unwrap()
        .parameters
        .ui_path
        .clone();
    assert_eq!(
        parent_ui,
        root.path().join("agent-drivers/probe/shared/latest/ui")
    );
    assert_eq!(child_ui, root.path().join("tools/probe/shared/latest/ui"));
    write(
        root.path(),
        "agent-drivers/probe/shared/latest/ui/index.html",
        "changed-source",
    );
    assert_eq!(
        agent_driver_core::read_driver_ui(&parent_ui, Default::default())
            .unwrap()
            .unwrap()
            .files[0]
            .data,
        b"changed-source"
    );
    let client = app.world_mut().spawn();
    app.world_mut().insert_component(
        client,
        address::Address::new("client:test/control").unwrap(),
    );
    assert!(matches!(
        command(&mut app, client, parent, "EFFECT agent_stop"),
        MclValue::Error(_)
    ));
    let services = app
        .world()
        .get_resource::<lua_runtime_core::LuaServices>()
        .unwrap()
        .clone();
    let vm = app
        .world()
        .get_component::<lua_runtime_core::DriverLuaVm>(execution)
        .unwrap();
    let identity = lua_runtime_core::LuaExecutionIdentity {
        source: execution,
        workspace: started.workspace,
        address: app
            .world()
            .get_component::<address::Address>(execution)
            .unwrap()
            .as_str()
            .into(),
        instance: vm.instance.start_id.clone(),
        cancel: vm.cancel.clone(),
    };
    let target = app
        .world()
        .get_component::<address::Address>(parent)
        .unwrap()
        .as_str()
        .to_owned();
    let (tx, mut rx) = oneshot::channel();
    async_runtime_plugin::WorldAsyncExt::send_async_closure(
        app.world(),
        app_runtime_plugin::RuntimePlugin::UPDATE,
        move || async move {
            let _ = tx.send(
                services
                    .call(identity, "agent_stop".into(), serde_json::json!(target))
                    .await,
            );
            Ok::<(), async_runtime_plugin::AsyncTaskError>(())
        },
    );
    assert_eq!(drive(&mut app, &mut rx).unwrap(), serde_json::json!({}));
    until(&mut app, |world| {
        !world.is_alive(parent) && !world.is_alive(child)
    });
    assert!(
        app.world()
            .get_component::<lua_runtime_core::DriverLuaVm>(execution)
            .is_some()
    );
    assert!(
        parent_home.join("state.sqlite").is_file() && child_home.join("state.sqlite").is_file()
    );
    let (tx, mut rx) = oneshot::channel();
    app.world().emit_event(StopWorkspace {
        id: "stop".into(),
        workspace: started.workspace,
        reply: Arc::new(Mutex::new(Some(tx))),
    });
    drive(&mut app, &mut rx).unwrap();
    assert!(
        app.world()
            .query_with::<lua_runtime_core::DriverLuaVm>()
            .result()
            .is_empty()
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(compose::shutdown_app(&mut app)).unwrap();
}
fn main() {
    if std::env::args()
        .nth(1)
        .is_some_and(|arg| arg.starts_with("--margatroid-tool-"))
    {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        std::process::exit(rt.block_on(tool_plugin::runner_main()));
    }
    let tests: Vec<(&str, fn())> = vec![(
        "identical_lua_programs_create_independent_agents_and_stop_without_owning_the_vm",
        identical_lua_programs_create_independent_agents_and_stop_without_owning_the_vm,
    )];
    for (_, test) in tests {
        test();
    }
}
