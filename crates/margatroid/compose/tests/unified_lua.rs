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
 assert(not pcall(function()return margatroid.agent:create({owner=owner,name='rejected-owner',config_path=context.resource_dir..'/setting.toml',ui_path=context.resource_dir..'/ui'})end))
 local agent=margatroid.agent:create({name=child and 'child' or context.name,config_path=context.resource_dir..'/setting.toml',ui_path=context.resource_dir..'/ui'})
 local data_dir=margatroid.agent:data_dir(agent.address)
 assert(type(data_dir)=='string' and data_dir:sub(1,1)=='/')
 assert(data_dir:sub(-#(child and '/agents/child' or '/agents/parent'))==(child and '/agents/child' or '/agents/parent'))
 assert(context.data_dir==nil and context.temp_dir==nil)
 assert(not pcall(function() return margatroid.agent:data_dir() end))
 assert(not pcall(function() return margatroid.agent:data_dir('agent:missing/none:latest') end))
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
        runner_path: std::env::var_os("MARGATROID_LIFETIME_TEST_BINARY")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::env::current_exe().unwrap()),
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
            .get_component::<address::Address>(parent)
            .unwrap()
            .as_str(),
        "agent:test/parent:latest"
    );
    assert_eq!(
        app.world()
            .get_component::<address::Address>(child)
            .unwrap()
            .as_str(),
        "agent:test/child:latest"
    );
    let binding = app
        .world()
        .get_component::<agent_core::AgentTaskBinding>(child)
        .unwrap();
    assert_eq!(binding.creator().address, "agent:test/parent:latest");
    assert!(!binding.creator().task_id.starts_with("task-instance:"));
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
    let parent_creator = app
        .world()
        .get_component::<agent_core::AgentTaskBinding>(parent)
        .unwrap()
        .creator();
    assert_eq!(parent_creator.address, "workspace:local/test:latest");
    assert_eq!(
        parent_creator.task_id,
        app.world()
            .get_component::<lua_runtime_core::DriverLuaVm>(execution)
            .unwrap()
            .instance
            .start_id
    );
    assert!(
        app.world()
            .get_component::<tool_core::ToolTasks>(parent)
            .unwrap()
            .get(
                &app.world()
                    .get_component::<agent_core::AgentTaskBinding>(child)
                    .unwrap()
                    .creator()
                    .task_id
            )
            .is_ok()
    );
    let parent_home = root.path().join("workspaces/test/agents/parent");
    let child_home = root.path().join("workspaces/test/agents/child");
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

fn creator_tasks_own_agents_until_explicit_stop_or_actual_task_cleanup() {
    for mode in [
        "normal",
        "error",
        "finish",
        "explicit",
        "parent-stop",
        "create-cancel",
    ] {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "config.toml", "[server]\nbind='127.0.0.1:0'\n");
        write(
            root.path(),
            "models.toml",
            "[[models]]\nid='offline'\nmodel='unused'\nprovider='openai'\napi_type='chat_completions'\nbase_url='http://127.0.0.1:1/v1'\napi_key='unused'\n",
        );
        for dir in ["tools", "prompts", "sandboxes", "skills"] {
            std::fs::create_dir_all(root.path().join(dir)).unwrap();
        }
        write(
            root.path(),
            "agent-drivers/probe/owner/latest/base.lua",
            r#"
function configure()return {timeout_ms=0}end
function execute(_,ctx)
 local agent=margatroid.agent:create({name=ctx.name,config_path=ctx.resource_dir..'/setting.toml',ui_path=ctx.resource_dir..'/ui'})
 assert(not margatroid.mcl:execute(agent.address,'IMPORT TOOL tool:probe/child:latest AS spawn_child').Error)
 assert(not margatroid.mcl:execute(agent.address,'EFFECT ready').Error)
 margatroid.task:return_result(agent.address)
 margatroid.task:receive()
end
"#,
        );
        write(
            root.path(),
            "agent-drivers/probe/owner/latest/setting.toml",
            "",
        );
        write(
            root.path(),
            "tools/probe/child/latest/tool.toml",
            "schema_version=1\ndescription='create task-owned agents'\n",
        );
        write(
            root.path(),
            "tools/probe/child/latest/input.schema.json",
            "{}",
        );
        write(root.path(), "tools/probe/child/latest/setting.toml", "");
        write(
            root.path(),
            "tools/probe/child/latest/main.lua",
            r#"
function configure()return {timeout_ms=0}end
function execute(_,ctx)
 local agents={}
 for i=1,2 do
  agents[i]=margatroid.agent:create({name='child'..i,config_path=ctx.resource_dir..'/setting.toml',ui_path=ctx.resource_dir..'/ui'})
  assert(not margatroid.mcl:execute(agents[i].address,'EFFECT ready').Error)
 end
 margatroid.task:return_result({task_id=ctx.task_id})
 local action=margatroid.task:receive()
 if action=='explicit' then
  for _,agent in ipairs(agents) do margatroid.agent:stop(agent.address) end
  margatroid.task:receive()
 elseif action=='error' then error('task-owner-failure') end
end
"#,
        );
        let definition = root.path().join("workspaces/test/workspace.yaml");
        write(
            root.path(),
            "workspaces/test/workspace.yaml",
            &format!(
                "name: test\nproject_root: {}\nmembers:\n- name: parent\n  driver: driver:probe/owner:latest\n  home_root: {}\n",
                root.path().display(),
                root.path().display()
            ),
        );
        let mut app = build_app(ApplicationOptions {
            data_root: root.path().into(),
            runner_path: std::env::var_os("MARGATROID_LIFETIME_TEST_BINARY")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::env::current_exe().unwrap()),
        })
        .unwrap();
        let (tx, mut rx) = oneshot::channel();
        app.world().emit_event(StartWorkspace {
            id: "start".into(),
            path: definition,
            reply: Arc::new(Mutex::new(Some(tx))),
        });
        if mode == "create-cancel" {
            until(&mut app, |world| {
                !world.query_with::<agent_core::Agent>().result().is_empty()
            });
            let source = app
                .world()
                .query_with::<lua_runtime_core::WorkspaceLuaTask>()
                .result()[0];
            let handle = app
                .world()
                .get_component::<tool_core::LuaExecutionTask>(source)
                .unwrap()
                .handle
                .clone();
            app.world()
                .get_component::<lua_runtime_core::DriverLuaVm>(source)
                .unwrap()
                .cancel
                .cancel();
            let _ = drive(&mut app, &mut rx);
            until(&mut app, |world| {
                handle.is_cleanup_complete()
                    && world.query_with::<agent_core::Agent>().result().is_empty()
            });
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(compose::shutdown_app(&mut app)).unwrap();
            continue;
        }
        let workspace = drive(&mut app, &mut rx).unwrap();
        let parent = app.world().query_with::<agent_core::Agent>().result()[0];
        let source = app.world_mut().spawn();
        app.world_mut().insert_component(
            source,
            address::Address::new("client:test/lifetime").unwrap(),
        );
        let (tx, mut rx) = oneshot::channel();
        app.world().emit_event(MclCommand {
            id: "spawn-task".into(),
            source,
            source_address: "client:test/lifetime:latest".into(),
            source_instance: None,
            target: parent,
            instruction: MclInstruction {
                text: "EFFECT tool_call ?".into(),
                arguments: vec![
                    serde_json::json!([{"id":"spawn","tool_name":"spawn_child","arguments":"{}"}]),
                ],
            },
            cancel: CancellationToken::new(),
            reply: Arc::new(Mutex::new(Some(tx))),
        });
        let result = drive(&mut app, &mut rx).unwrap();
        assert!(!matches!(result, MclValue::Error(_)), "{result:?}");
        let tasks = app
            .world()
            .get_component::<tool_core::ToolTasks>(parent)
            .unwrap()
            .clone();
        let (task_id, task) = tasks
            .list()
            .into_iter()
            .find(|(_, handle)| handle.has_returned())
            .unwrap();
        let children = app
            .world()
            .query_with::<agent_core::Agent>()
            .result()
            .into_iter()
            .filter(|entity| *entity != parent)
            .collect::<Vec<_>>();
        assert_eq!(children.len(), 2);
        assert!(!task.is_cleanup_complete());
        let mut stop_reply = None;
        if mode == "parent-stop" {
            let (tx, rx) = oneshot::channel();
            app.world().emit_event(agent_core::StopAgent {
                id: "parent-stop".into(),
                agent: parent,
                reply: Arc::new(Mutex::new(Some(tx))),
            });
            stop_reply = Some(rx);
        } else if mode == "finish" {
            let child_tasks = app
                .world()
                .get_component::<tool_core::ToolTasks>(children[0])
                .unwrap()
                .clone();
            let (tx, _rx) = oneshot::channel();
            let (held, _input) = tool_core::ToolTaskHandle::new(
                &tool_core::ToolExecutionLimits::default(),
                "held".into(),
                "held".into(),
                tx,
                child_tasks.update_context(),
                None,
            )
            .unwrap();
            child_tasks.register("held".into(), held.clone()).unwrap();
            let (release, wait) = oneshot::channel();
            let cleanup = held.clone();
            let runtime = tokio::runtime::Runtime::new().unwrap();
            held.attach_task(runtime.spawn(async move {
                wait.await.unwrap();
                child_tasks.complete_cleanup("held", &cleanup, None);
            }))
            .unwrap();
            let finishing = task.clone();
            let (tx, mut rx) = oneshot::channel();
            async_runtime_plugin::WorldAsyncExt::send_async_closure(
                app.world(),
                app_runtime_plugin::RuntimePlugin::UPDATE,
                move || async move {
                    let result = finishing.finish().await;
                    let _ = tx.send(result);
                    Ok::<(), async_runtime_plugin::AsyncTaskError>(())
                },
            );
            until(&mut app, |_| held.cancel_token().is_cancelled());
            assert!(rx.try_recv().is_err());
            assert!(!task.is_cleanup_complete());
            assert!(app.world().is_alive(children[0]));
            release.send(()).unwrap();
            drive(&mut app, &mut rx).unwrap();
            assert!(children.iter().all(|entity| !app.world().is_alive(*entity)));
        } else {
            tasks.send(&task_id, serde_json::json!(mode)).unwrap();
        }
        until(&mut app, |world| {
            children.iter().all(|entity| !world.is_alive(*entity))
        });
        if mode == "explicit" {
            assert!(!task.is_cleanup_complete());
            let task = task.clone();
            let (tx, mut rx) = oneshot::channel();
            async_runtime_plugin::WorldAsyncExt::send_async_closure(
                app.world(),
                app_runtime_plugin::RuntimePlugin::UPDATE,
                move || async move {
                    let _ = tx.send(task.finish().await);
                    Ok::<(), async_runtime_plugin::AsyncTaskError>(())
                },
            );
            drive(&mut app, &mut rx).unwrap();
        }
        if let Some(mut rx) = stop_reply {
            drive(&mut app, &mut rx).unwrap();
        }
        until(&mut app, |_| task.is_cleanup_complete());
        assert!(children.iter().all(|entity| !app.world().is_alive(*entity)));
        if mode != "parent-stop" {
            assert!(app.world().is_alive(parent));
        }
        let (tx, mut rx) = oneshot::channel();
        app.world().emit_event(StopWorkspace {
            id: "stop-workspace".into(),
            workspace: workspace.workspace,
            reply: Arc::new(Mutex::new(Some(tx))),
        });
        drive(&mut app, &mut rx).unwrap();
        assert!(
            app.world()
                .query_with::<agent_core::Agent>()
                .result()
                .is_empty()
        );
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(compose::shutdown_app(&mut app)).unwrap();
    }
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
    creator_tasks_own_agents_until_explicit_stop_or_actual_task_cleanup();
    let tests: Vec<(&str, fn())> = vec![(
        "identical_lua_programs_create_independent_agents_and_stop_without_owning_the_vm",
        identical_lua_programs_create_independent_agents_and_stop_without_owning_the_vm,
    )];
    for (_, test) in tests {
        test();
    }
}
