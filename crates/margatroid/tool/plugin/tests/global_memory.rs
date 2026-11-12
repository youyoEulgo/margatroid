#![cfg(target_os = "linux")]

#[path = "support/storage.rs"]
mod storage_fixture;

use agent_core::{Agent, AgentState};
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin};
use core_plugin::{App, Entity};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::oneshot;
use tool_core::{
    InitToolTasks, ToolError, ToolExecuteRequest, ToolExecuteResponse, ToolExecutionLimits,
    ToolTaskHandle, ToolTasks,
};
use tool_plugin::ToolPlugin;
use tool_resource_core::{read_tool, ToolEntry, Tools};
use workspace_core::{Workspace, WorkspaceState};

const MIB: usize = 1024 * 1024;
const TASK_CAP: usize = 128 * MIB;
const TOTAL_CAP: usize = 96 * MIB;
const BUDGET: Duration = Duration::from_secs(10);
type Reply = oneshot::Receiver<Result<ToolExecuteResponse, ToolError>>;

// No subprocess or allocation until the host verifies this ordinary IO proof.
const PROGRAM: &str = r#"
function configure() return {timeout_ms=10000} end
function execute(args, context)
    local function read(path) return margatroid.files:read(path):match('^%s*(.-)%s*$') end
    local relative=read('/proc/self/cgroup'):match('0::([^\n]+)')
    local group='/sys/fs/cgroup'..relative
    local stat=read('/proc/self/stat')
    local pid=tonumber(stat:match('^(%d+)'))
    local ppid=tonumber(stat:match('^%d+ %b() %S+ (%d+)'))
    local parent=group:match('^(.*)/[^/]+$')
    local ancestors={}
    while parent and parent~='/sys/fs' do
        local ok, maximum=pcall(read,parent..'/memory.max')
        if ok then ancestors[#ancestors+1]={path=parent,max=maximum,oom_group=read(parent..'/memory.oom.group')} end
        if parent=='/sys/fs/cgroup' then break end
        parent=parent:match('^(.*)/[^/]+$')
    end
    local proof={pid=pid,ppid=ppid,group=group,max=read(group..'/memory.max'),
        swap=read(group..'/memory.swap.max'),oom_group=read(group..'/memory.oom.group'),ancestors=ancestors}
    margatroid.files:write(args.proof,margatroid.data:encode('json', proof))
    margatroid.task:return_result('ready')
    local command=margatroid.task:receive()
    if args.thin then
        margatroid.task:append_output(command)
        margatroid.task:receive()
        return
    end
    assert(command=='allocate')
    local python=[[import json,os,pathlib,sys
pathlib.Path('/proc/self/oom_score_adj').write_text('1000')
pathlib.Path(sys.argv[1]).write_text(json.dumps({'pid':os.getpid(),'group':pathlib.Path('/proc/self/cgroup').read_text()}))
data=bytearray(int(sys.argv[2]))
for offset in range(0,len(data),4096): data[offset]=1
print('allocated',flush=True)
sys.stdin.readline()
]]
    local child=margatroid.process:start('/usr/bin/python3', {'-u','-c',python,args.child_proof,tostring(args.bytes)})
    local frame=child:read()
    assert(frame.data:find('allocated',1,true),'allocator exited before acknowledgement')
    margatroid.task:append_output({allocated=args.bytes})
    margatroid.task:receive()
    child:close()
end
"#;

struct Fixture {
    app: App,
    agents: [Entity; 2],
    root: tempfile::TempDir,
    handles: Vec<ToolTaskHandle>,
    deadline: Instant,
    finished: bool,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let resource = root.path().join("memory/latest");
        std::fs::create_dir_all(&resource).unwrap();
        std::fs::write(
            resource.join("tool.toml"),
            "schema_version=1\ndescription='real global memory test'\n",
        )
        .unwrap();
        std::fs::write(resource.join("input.schema.json"), "{}").unwrap();
        std::fs::write(resource.join("main.lua"), PROGRAM).unwrap();
        let imported = read_tool(&resource.join("tool.toml")).unwrap();
        let mut tools = Tools::default();
        tools
            .insert(
                "memory".into(),
                ToolEntry {
                    path: imported.origin,
                    description: imported.description,
                    input_schema: imported.input_schema,
                    owner_task_id: None,
                    binding: None,
                },
            )
            .unwrap();
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(AsyncRuntimePlugin)
            .add_plugin(ClosurePlugin)
            .add_closure_system(RuntimePlugin::UPDATE)
            .add_plugin(
                ToolPlugin::new()
                    .with_runner_path(env!("CARGO_BIN_EXE_tool_runner"))
                    .with_limits(ToolExecutionLimits {
                        memory_bytes: TASK_CAP,
                        total_memory_bytes: TOTAL_CAP,
                        cache_bytes: 64,
                    }),
            );
        let workspace = app.world_mut().spawn();
        app.world_mut().insert_component(
            workspace,
            Workspace {
                definition: None,
                name: "global-memory".into(),
                definition_path: root.path().join("workspace.yaml"),
                project_root: root.path().to_owned(),
                state: WorkspaceState::Running,
                agents: BTreeSet::new(),
            },
        );
        let agents = std::array::from_fn(|index| {
            let agent = app.world_mut().spawn();
            storage_fixture::mount(app.world_mut(), agent, root.path());
            app.world_mut().insert_component(
                agent,
                Agent {
                    name: format!("memory-{index}"),
                    workspace,
                    state: AgentState::Working,
                    turn: Some("test-turn".into()),
                },
            );
            app.world_mut().insert_component(agent, tools.clone());
            let (sender, mut reply) = oneshot::channel();
            app.world().emit_event(InitToolTasks {
                agent,
                reply: Arc::new(Mutex::new(Some(sender))),
            });
            app.tick();
            assert!(reply.try_recv().unwrap().is_ok());
            agent
        });
        Self {
            app,
            agents,
            root,
            handles: vec![],
            deadline: Instant::now() + BUDGET,
            finished: false,
        }
    }

    fn tasks(&self, agent: usize) -> ToolTasks {
        self.app
            .world()
            .get_component::<ToolTasks>(self.agents[agent])
            .unwrap()
            .clone()
    }

    fn execute(&mut self, agent: usize, id: &str, bytes: usize, thin: bool) -> Reply {
        let (sender, reply) = oneshot::channel();
        self.app.world().emit_event(ToolExecuteRequest { id: id.into(), agent: self.agents[agent],
            turn: "test-turn".into(), alias: "memory".into(), call_id: format!("call-{id}"),
            arguments: json!({"proof": self.root.path().join(format!("{id}.json")),
                "child_proof": self.root.path().join(format!("{id}-child.json")), "bytes": bytes, "thin": thin}),
            reply: Arc::new(Mutex::new(Some(sender))) });
        self.app.tick();
        self.app.tick();
        self.handles.push(self.tasks(agent).get(id).unwrap());
        reply
    }

    fn tick(&mut self) {
        self.app.tick();
        assert!(
            Instant::now() < self.deadline,
            "global memory test exceeded 10s"
        );
        std::thread::sleep(Duration::from_millis(2));
    }

    fn ready(&mut self, mut reply: Reply) {
        loop {
            self.tick();
            match reply.try_recv() {
                Ok(result) => {
                    let response = result.unwrap();
                    assert!(!response.failed, "{}", response.content);
                    assert_eq!(response.content, "ready");
                    return;
                }
                Err(oneshot::error::TryRecvError::Empty) => {}
                Err(error) => panic!("tool reply closed: {error}"),
            }
        }
    }

    fn proof(&self, id: &str) -> Value {
        serde_json::from_slice(&std::fs::read(self.root.path().join(format!("{id}.json"))).unwrap())
            .unwrap()
    }

    fn verify_boundary(&self, ids: &[&str]) -> PathBuf {
        let mut shared = None;
        let outside = std::fs::read_to_string("/proc/self/cgroup").unwrap();
        for id in ids {
            let proof = self.proof(id);
            assert_eq!(proof["max"], TASK_CAP.to_string());
            assert_eq!(proof["swap"], "0");
            assert_eq!(proof["oom_group"], "1");
            let parent = proof["ancestors"]
                .as_array()
                .unwrap()
                .iter()
                .find(|ancestor| {
                    ancestor["max"]
                        .as_str()
                        .and_then(|value| value.parse::<usize>().ok())
                        == Some(TOTAL_CAP)
                })
                .expect("no actual ancestor enforces the configured global cap");
            assert_eq!(parent["oom_group"], "0");
            let path = PathBuf::from(parent["path"].as_str().unwrap());
            assert!(Path::new(proof["group"].as_str().unwrap()).starts_with(&path));
            if let Some(ref first) = shared {
                assert_eq!(first, &path, "agents do not share their memory parent");
            }
            shared = Some(path.clone());
            let supervisor = proof["ppid"].as_u64().unwrap();
            let supervisor_group =
                std::fs::read_to_string(format!("/proc/{supervisor}/cgroup")).unwrap();
            let relative = path
                .strip_prefix("/sys/fs/cgroup")
                .unwrap()
                .to_str()
                .unwrap();
            assert!(
                !supervisor_group.contains(relative),
                "trusted supervisor charged to global pool"
            );
            assert!(
                !outside.contains(relative),
                "test host charged to global pool"
            );
        }
        shared.unwrap()
    }

    fn allocated(&mut self, agent: usize, id: &str, bytes: usize) {
        loop {
            self.tick();
            let output = self.tasks(agent).read(id).unwrap();
            assert!(output.error.is_none(), "{id}: {:?}", output.error);
            if !output.output.is_empty() {
                assert_eq!(output.output, [json!({"allocated": bytes})]);
                return;
            }
        }
    }

    // Runs during unwinding too: cancel all handles, await supervision, then
    // check every PID recorded before allocation rather than guessing names.
    fn finish(&mut self) {
        if self.finished {
            return;
        }
        for handle in &self.handles {
            handle.cancel();
        }
        let handles = self.handles.clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        let cleanup = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
                let _ = tokio::time::timeout(
                    BUDGET,
                    futures_util::future::join_all(handles.iter().map(ToolTaskHandle::finish)),
                )
                .await;
            });
            let _ = sender.send(());
        });
        let deadline = Instant::now() + BUDGET;
        while receiver.try_recv().is_err() && Instant::now() < deadline {
            self.app.tick();
            std::thread::sleep(Duration::from_millis(2));
        }
        let _ = cleanup.join();
        let mut surviving = vec![];
        for entry in std::fs::read_dir(self.root.path()).unwrap().flatten() {
            if entry.path().extension().and_then(|v| v.to_str()) != Some("json") {
                continue;
            }
            if let Ok(bytes) = std::fs::read(entry.path()) {
                if let Ok(proof) = serde_json::from_slice::<Value>(&bytes) {
                    if let Some(pid) = proof["pid"].as_u64() {
                        if Path::new(&format!("/proc/{pid}")).exists() {
                            surviving.push(pid);
                        }
                    }
                }
            }
        }
        self.finished = true;
        let complete = self.handles.iter().all(ToolTaskHandle::is_cleanup_complete);
        if std::thread::panicking() {
            if !surviving.is_empty() || !complete {
                eprintln!(
                    "cleanup failed during unwinding: live PIDs {surviving:?}; all tasks complete: {complete}"
                );
            }
        } else {
            assert!(
                surviving.is_empty(),
                "owned processes survived cleanup: {surviving:?}"
            );
            assert!(complete, "task supervision did not prove cleanup");
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.finish();
    }
}

#[test]
fn shared_global_memory_allows_two_agents_when_children_fit_and_keeps_per_task_buffers() {
    let mut f = Fixture::new();
    let first = f.execute(0, "first", 8 * MIB, false);
    let second = f.execute(1, "second", 8 * MIB, false);
    f.ready(first);
    f.ready(second);
    let parent = f.verify_boundary(&["first", "second"]);
    f.tasks(0).send("first", json!("allocate")).unwrap();
    f.tasks(1).send("second", json!("allocate")).unwrap();
    f.allocated(0, "first", 8 * MIB);
    f.allocated(1, "second", 8 * MIB);
    // The 64-byte cache budget applies to each task, not to their sum.
    let value = json!("x".repeat(48));
    for handle in &f.handles {
        handle.write(value.clone()).unwrap();
    }
    assert_eq!(
        f.tasks(0).read("first").unwrap().output.as_slice(),
        std::slice::from_ref(&value)
    );
    assert_eq!(f.tasks(1).read("second").unwrap().output, [value]);
    assert_eq!(counter(&parent, "oom_kill"), 0);
    f.finish();
}

fn counter(parent: &Path, name: &str) -> u64 {
    std::fs::read_to_string(parent.join("memory.events"))
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{name} ")))
        .unwrap()
        .parse()
        .unwrap()
}

#[test]
fn shared_global_memory_oom_crosses_agents_below_each_task_cap() {
    let mut f = Fixture::new();
    let first = f.execute(0, "first", 36 * MIB, false);
    let second = f.execute(1, "second", 60 * MIB, false);
    let thin = f.execute(0, "thin", 0, true);
    f.ready(first);
    f.ready(second);
    f.ready(thin);
    let parent = f.verify_boundary(&["first", "second", "thin"]);
    let before = counter(&parent, "oom_kill");
    f.tasks(0).send("first", json!("allocate")).unwrap();
    f.allocated(0, "first", 36 * MIB);
    f.tasks(1).send("second", json!("allocate")).unwrap();
    loop {
        f.tick();
        if counter(&parent, "oom_kill") > before {
            break;
        }
    }
    // Victim count/identity is kernel policy; the parent must not group-kill.
    assert_eq!(
        std::fs::read_to_string(parent.join("memory.oom.group"))
            .unwrap()
            .trim(),
        "0"
    );
    loop {
        f.tick();
        if f.handles[..2]
            .iter()
            .any(|handle| handle.is_cleanup_complete())
        {
            break;
        }
    }
    assert!(counter(&parent, "max") > 0);
    f.finish();
}
