#![allow(dead_code)]
use address::{Address, WorldAddressExt};
use agent_core::{Agent, StopAgent};
use app_runtime_plugin::WorldEventExt;
use compose::{ApplicationOptions, build_app};
use core_plugin::{App, Entity, World};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use storage_core::{AgentStorage, StorageOperation, StorageValue};
use tokio::sync::oneshot;
use tool_core::{ToolExecuteRequest, ToolExecuteResponse, ToolTaskHandle, ToolTasks};
use workspace_core::{CreateWorkspace, Workspace};

pub const DRIVER: &str = r#"
function configure() return {timeout_ms=0} end
function execute(args,context)
 assert(context.temp_dir==nil and context.data_dir==nil)
 assert(not pcall(function()margatroid.agent:create({owner='forged',node_name='invalid'})end))
 local agent=margatroid.agent:create({node_name=args.node_name,alias=args.alias,project_root=context.project_root,config_path=args.config_path or context.resource_dir..'/setting.toml',ui_path=args.ui_path or context.resource_dir..'/ui'})
 local function command(text,values)
  local result=margatroid.mcl:execute(agent.address,text,values)
  if result.Error then error(result.Error.message)end
  return result
 end
 assert(type(margatroid.agent:data_dir(agent.address))=='string')
 assert(not pcall(function()margatroid.agent:data_dir()end))
 local empty=command('CREATE BLOCK saved (messages)')
 assert(margatroid.data:encode('json',empty)=='{}')
 local bad=margatroid.mcl:execute(agent.address,'INVALID_MCL_INSTRUCTION')
 assert(type(bad.Error.message)=='string')
 command('CREATE TEMP BLOCK transient (messages)')
 local previous=command('GET transient.messages').Array.Message
 assert(#previous==0,'temporary messages reused across actual Agent instances')
 command('INJECT ? TO transient.messages',{{{type='user',content='temporary'}}})
 command('EFFECT ready')
 if args.append then command('INJECT ? TO saved.messages',{{{type='user',content=args.append}}}) end
 if args.model then
  local model=command('EFFECT model_set ?',{{model='offline'}}).Config
  assert(model.context_window_tokens==128000)
 end
 margatroid.task:return_result(agent.address)
 margatroid.task:receive()
end
"#;

pub fn write(root: &Path, name: impl AsRef<Path>, text: &str) {
    let path = root.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}
pub fn runner() -> PathBuf {
    if let Some(path) = std::env::var_os("MARGATROID_TEST_RUNNER") {
        return PathBuf::from(path);
    }
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .unwrap()
        .join("target/debug/margatroid");
    assert!(
        path.is_file(),
        "build the real test runner first: cargo build -p margatroid, or set MARGATROID_TEST_RUNNER"
    );
    path
}
pub fn drive<T>(app: &mut App, rx: &mut oneshot::Receiver<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        app.tick();
        match rx.try_recv() {
            Ok(value) => return value,
            Err(oneshot::error::TryRecvError::Empty) => {}
            Err(e) => panic!("reply closed: {e}"),
        };
        assert!(Instant::now() < deadline, "operation timed out");
        std::thread::sleep(Duration::from_millis(1));
    }
}
pub fn until(app: &mut App, predicate: impl Fn(&World) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !predicate(app.world()) {
        app.tick();
        assert!(Instant::now() < deadline, "lifecycle timed out");
        std::thread::sleep(Duration::from_millis(1));
    }
}
pub struct Harness {
    pub app: App,
    pub root: tempfile::TempDir,
    pub workspace: Entity,
}
impl Harness {
    pub fn open() -> Self {
        Self::with_runner(runner())
    }
    pub fn with_runner(runner_path: PathBuf) -> Self {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "config.toml", "[server]\nbind='127.0.0.1:0'\n");
        write(
            root.path(),
            "models.toml",
            "[[models]]\nid='offline'\nmodel='unused'\nprovider='openai'\napi_type='chat_completions'\nbase_url='http://127.0.0.1:1/v1'\napi_key='unused'\ncontext_window='128k'\n",
        );
        for directory in [
            "project",
            "workspaces",
            "tools",
            "prompts",
            "sandboxes",
            "skills",
        ] {
            std::fs::create_dir_all(root.path().join(directory)).unwrap();
        }
        let mut app = build_app(ApplicationOptions {
            data_root: root.path().into(),
            runner_path,
        })
        .unwrap();
        let (tx, mut rx) = oneshot::channel();
        app.world().send_event(CreateWorkspace {
            address: "/demo".into(),
            alias: "Demo".into(),
            project_root: root.path().join("project"),
            reply: Arc::new(Mutex::new(Some(tx))),
        });
        let workspace = drive(&mut app, &mut rx).unwrap();
        Self {
            app,
            root,
            workspace,
        }
    }
    pub fn driver(&self, name: &str, program: &str) -> PathBuf {
        let folder = PathBuf::from(format!("tools/probe/{name}/dev"));
        write(
            self.root.path(),
            folder.join("tool.toml"),
            "schema_version=1\ndescription='Agent lifecycle driver'\n",
        );
        write(
            self.root.path(),
            folder.join("input.schema.json"),
            "{\"type\":\"object\"}",
        );
        write(self.root.path(), folder.join("main.lua"), program);
        write(
            self.root.path(),
            folder.join("setting.toml"),
            "label='initial'\n",
        );
        write(
            self.root.path(),
            folder.join("ui/index.html"),
            "<main>driver-ui</main>",
        );
        self.root.path().join(folder).join("tool.toml")
    }
    pub fn execute(
        &mut self,
        parent: Entity,
        path: &Path,
        arguments: Value,
    ) -> Result<ToolExecuteResponse, tool_core::ToolError> {
        let (tx, mut rx) = oneshot::channel();
        self.app.world().send_event(ToolExecuteRequest::Path {
            initiator: parent,
            path: path.into(),
            arguments,
            reply: Arc::new(Mutex::new(Some(tx))),
        });
        drive(&mut self.app, &mut rx)
    }
    pub fn start(&mut self, parent: Entity, path: &Path, arguments: Value) -> Entity {
        let response = self.execute(parent, path, arguments).unwrap();
        assert!(!response.failed, "{response:?}");
        let address = response.content;
        let entity = self.app.world().entity_by_address(&address).unwrap();
        until(&mut self.app, |world| {
            world
                .get_component::<Agent>(entity)
                .is_some_and(|a| a.state() == agent_core::AgentState::Running)
        });
        entity
    }
    pub fn task(&self, agent: Entity) -> ToolTaskHandle {
        let agent = self.app.world().get_component::<Agent>(agent).unwrap();
        self.app
            .world()
            .get_component::<ToolTasks>(agent.parent())
            .unwrap()
            .get(agent.task_id())
            .unwrap()
    }
    pub fn stop(&mut self, agent: Entity) {
        let address = self
            .app
            .world()
            .get_component::<Address>(agent)
            .unwrap()
            .as_str()
            .to_owned();
        let (tx, mut rx) = oneshot::channel();
        self.app.world().send_event(StopAgent {
            address,
            reply: Arc::new(Mutex::new(Some(tx))),
        });
        drive(&mut self.app, &mut rx).unwrap();
    }
    pub fn finish(&mut self, task: ToolTaskHandle) {
        let (tx, mut rx) = oneshot::channel();
        self.app
            .world()
            .get_resource::<async_runtime_plugin::AsyncRuntimeHandle>()
            .unwrap()
            .spawn(async move {
                let _ = tx.send(task.finish().await);
            });
        drive(&mut self.app, &mut rx).unwrap();
    }
    pub fn config(&self, agent: Entity) -> toml::Table {
        let storage = self
            .app
            .world()
            .get_component::<AgentStorage>(agent)
            .unwrap();
        let StorageValue::Config(value) =
            storage_core::execute_storage(storage, StorageOperation::ReadConfig).unwrap()
        else {
            panic!("unexpected storage reply")
        };
        value
    }
    pub fn close(mut self) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(compose::shutdown_app(&mut self.app)).unwrap();
    }
}
fn nested_creators_own_actual_task_identity_and_close_descendants(runner: PathBuf) {
    let mut h = Harness::with_runner(runner);
    let path = h.driver("shared", DRIVER);
    let parent = h.start(
        h.workspace,
        &path,
        json!({"node_name":"parent","append":"parent-data","model":true}),
    );
    let child = h.start(
        parent,
        &path,
        json!({"node_name":"child","append":"child-data"}),
    );
    let parent_agent = h.app.world().get_component::<Agent>(parent).unwrap();
    assert_eq!(parent_agent.parent(), h.workspace);
    let child_agent = h.app.world().get_component::<Agent>(child).unwrap();
    assert_eq!(child_agent.parent(), parent);
    let task = child_agent.task_id().to_owned();
    assert!(
        h.app
            .world()
            .get_component::<ToolTasks>(parent)
            .unwrap()
            .get(&task)
            .is_ok()
    );
    assert_eq!(
        h.app
            .world()
            .get_component::<Address>(child)
            .unwrap()
            .as_str(),
        "/demo/parent/child"
    );
    let database = h
        .app
        .world()
        .get_component::<Workspace>(h.workspace)
        .unwrap()
        .database
        .clone();
    assert_eq!(
        database.agent_by_node("child").unwrap().parent_node,
        "/demo/parent"
    );
    let parent_task = h.task(parent);
    let child_task = h.task(child);
    h.stop(parent);
    assert!(!h.app.world().is_alive(child));
    assert!(parent_task.is_cleanup_complete());
    assert!(child_task.is_cleanup_complete());
    assert!(database.agent_by_node("parent").is_ok());
    assert!(database.agent_by_node("child").is_ok());
    h.close();
}
fn same_task_closes_all_created_agents(runner: PathBuf) {
    let mut h = Harness::with_runner(runner);
    let path=h.driver("siblings",r#"
function configure()return{timeout_ms=0}end
function execute(args,context)
 local addresses={}
 for _,name in ipairs({'alpha','beta'})do
  local a=margatroid.agent:create({node_name=name,project_root=context.project_root,config_path=context.resource_dir..'/setting.toml',ui_path=context.resource_dir..'/ui'})
  local ready=margatroid.mcl:execute(a.address,'EFFECT ready');assert(not ready.Error)
  addresses[#addresses+1]=a.address
 end
 margatroid.task:return_result(addresses)
 margatroid.task:receive()
end
"#);
    let reply = h.execute(h.workspace, &path, json!({})).unwrap();
    assert!(!reply.failed);
    let addresses: Vec<String> = serde_json::from_str(&reply.content).unwrap();
    assert_eq!(addresses.len(), 2);
    let alpha = h.app.world().entity_by_address(&addresses[0]).unwrap();
    let beta = h.app.world().entity_by_address(&addresses[1]).unwrap();
    assert_eq!(
        h.app
            .world()
            .get_component::<Agent>(alpha)
            .unwrap()
            .task_id(),
        h.app
            .world()
            .get_component::<Agent>(beta)
            .unwrap()
            .task_id()
    );
    h.stop(alpha);
    assert!(!h.app.world().is_alive(beta));
    h.close();
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
    let runner = std::env::var_os("MARGATROID_TEST_RUNNER")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_exe().unwrap());
    nested_creators_own_actual_task_identity_and_close_descendants(runner.clone());
    same_task_closes_all_created_agents(runner);
}
