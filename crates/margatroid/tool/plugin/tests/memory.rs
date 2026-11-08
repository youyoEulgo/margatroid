#![cfg(target_os = "linux")]

use serde_json::{json, Value};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tool_core::ToolExecutionLimits;

const MIB: usize = 1024 * 1024;

struct SupervisorGuard {
    child: Option<tokio::process::Child>,
    root: std::path::PathBuf,
}

impl Drop for SupervisorGuard {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        let root = self.root.clone();
        let cleaned = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
                // The protocol stdin has already dropped; EOF asks the trusted
                // supervisor to clean up its descendants before it exits.
                if tokio::time::timeout(Duration::from_secs(5), child.wait())
                    .await
                    .is_err()
                {
                    for name in ["first", "second"] {
                        if let Ok(bytes) = std::fs::read(root.join(format!("{name}.json"))) {
                            if let Ok(proof) = serde_json::from_slice::<Value>(&bytes) {
                                if let (Some(pid), Some(group)) =
                                    (proof["pid"].as_u64(), proof["group"].as_str())
                                {
                                    let relative =
                                        group.strip_prefix("/sys/fs/cgroup").unwrap_or(group);
                                    if std::fs::read_to_string(format!("/proc/{pid}/cgroup"))
                                        .is_ok_and(|value| value.contains(relative))
                                    {
                                        let _ = nix::sys::signal::kill(
                                            nix::unistd::Pid::from_raw(pid as i32),
                                            nix::sys::signal::Signal::SIGKILL,
                                        );
                                    }
                                }
                            }
                        }
                    }
                    let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
                }
            });
            ["first", "second"].iter().all(|name| {
                let Ok(bytes) = std::fs::read(root.join(format!("{name}.json"))) else {
                    return true;
                };
                let Ok(proof) = serde_json::from_slice::<Value>(&bytes) else {
                    return true;
                };
                !proof["pid"]
                    .as_u64()
                    .is_some_and(|pid| Path::new(&format!("/proc/{pid}")).exists())
            })
        })
        .join()
        .unwrap_or(false);
        if std::thread::panicking() {
            if !cleaned {
                eprintln!("memory test failed to reap all owned allocator PIDs");
            }
        } else {
            assert!(
                cleaned,
                "memory test failed to reap all owned allocator PIDs"
            );
        }
    }
}

async fn send(input: &mut tokio::process::ChildStdin, frame: Value) {
    input
        .write_all((frame.to_string() + "\n").as_bytes())
        .await
        .unwrap();
}

async fn receive(output: &mut BufReader<tokio::process::ChildStdout>) -> Value {
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(10), output.read_line(&mut line))
        .await
        .expect("runner frame timeout")
        .expect("runner read failed");
    assert!(
        !line.is_empty(),
        "runner closed before cleanup confirmation"
    );
    serde_json::from_str(&line).unwrap()
}

fn program(allocation: usize, grandchild: bool) -> String {
    let code = format!(
        r#"import json,os,pathlib,sys,time
if sys.argv[2] == 'yes': os.setsid()
relative = pathlib.Path('/proc/self/cgroup').read_text().strip().split('::',1)[1]
group = pathlib.Path('/sys/fs/cgroup') / relative.lstrip('/')
proof = {{'pid':os.getpid(), 'group':str(group), 'max':(group/'memory.max').read_text().strip(), 'swap':(group/'memory.swap.max').read_text().strip(), 'oom_group':(group/'memory.oom.group').read_text().strip()}}
pathlib.Path(sys.argv[1]).write_text(json.dumps(proof))
print('ready',flush=True)
sys.stdin.readline()
data=bytearray({allocation})
for offset in range(0,len(data),4096): data[offset]=1
print('allocated',flush=True)
sys.stdin.readline()
"#
    );
    let launcher = if grandchild {
        "import subprocess,sys; child=subprocess.Popen([sys.executable,'-u','-c',sys.argv[1],sys.argv[2],'yes']); sys.exit(child.wait())"
    } else {
        "import sys; code=sys.argv[1]; sys.argv=[sys.argv[0],sys.argv[2],'no']; exec(compile(code,'allocator','exec'))"
    };
    format!(
        r#"
function configure() return {{timeout_ms=5000}} end
function execute()
    local code={code}
    local function spawn(name)
        return margatroid.process:start('/usr/bin/python3', {{'-u','-c',{launcher},code,margatroid.context.project_root..'/'..name..'.json'}})
    end
    local first=spawn('first')
    assert(first:read().data:find('ready',1,true))
    margatroid.task:return_result('boundary-ready')
    assert(margatroid.task:receive()=='allocate')
    first:write('allocate\n')
    assert(first:read().data:find('allocated',1,true))
    local second=spawn('second')
    assert(second:read().data:find('ready',1,true))
    second:write('allocate\n')
    assert(second:read().data:find('allocated',1,true))
    first:close()
    second:close()
end
"#,
        code = serde_json::to_string(&code).unwrap(),
        launcher = serde_json::to_string(launcher).unwrap()
    )
}

async fn run(allocation: usize, cap: usize, grandchild: bool) -> (tempfile::TempDir, Value) {
    let root = tempfile::tempdir().unwrap();
    let outside = std::fs::read_to_string("/proc/self/cgroup").unwrap();
    let child = tokio::process::Command::new(env!("CARGO_BIN_EXE_tool_runner"))
        .arg("--margatroid-tool-supervisor")
        .current_dir(root.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let supervisor_pid = child.id().unwrap();
    let mut supervisor = SupervisorGuard {
        child: Some(child),
        root: root.path().to_owned(),
    };
    let mut input = supervisor.child.as_mut().unwrap().stdin.take().unwrap();
    let mut output = BufReader::new(supervisor.child.as_mut().unwrap().stdout.take().unwrap());
    send(
        &mut input,
        json!({
            "protocol_version":1,"program":program(allocation,grandchild),"arguments":{},
            "context":{"agent_name":"protocol-fixture","turn":"test-turn","source_origin":"protocol-fixture.lua","task_id":"memory-test","call_id":"memory-call","alias":"memory",
                "resource_dir":root.path(),"project_root":root.path(),"temp_dir":root.path()},
            "limits":ToolExecutionLimits {memory_bytes:cap,..Default::default()}
        }),
    )
    .await;
    let configured = receive(&mut output).await;
    assert_eq!(configured["frame"]["type"], "configured", "{configured}");
    assert_eq!(
        std::fs::read_to_string(format!("/proc/{supervisor_pid}/cgroup")).unwrap(),
        outside,
        "trusted supervisor must remain outside the task's memory group"
    );
    send(
        &mut input,
        json!({"protocol_version":1,"frame":{"type":"configured"}}),
    )
    .await;
    let stopped = loop {
        let wire = receive(&mut output).await;
        let frame = &wire["frame"];
        match frame["type"].as_str() {
            Some("return") => {
                assert_eq!(frame["value"], "boundary-ready");
                let proof: Value =
                    serde_json::from_slice(&std::fs::read(root.path().join("first.json")).unwrap())
                        .unwrap();
                assert_eq!(proof["max"], cap.to_string());
                assert_eq!(proof["swap"], "0");
                assert_eq!(proof["oom_group"], "1");
                send(
                    &mut input,
                    json!({"protocol_version":1,"frame":{
                        "type":"response","request_id":frame["request_id"],"result":{"Ok":null}
                    }}),
                )
                .await;
                send(
                    &mut input,
                    json!({"protocol_version":1,"frame":{
                        "type":"input","value":"allocate"
                    }}),
                )
                .await;
            }
            Some("input_consumed") => {}
            Some("stopped") => break wire,
            other => panic!("unexpected memory-test frame {other:?}: {wire}"),
        }
    };
    assert_eq!(stopped["frame"]["cleanup_proven"], true, "{stopped}");
    tokio::time::timeout(
        Duration::from_secs(5),
        supervisor.child.as_mut().unwrap().wait(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        std::fs::read_to_string("/proc/self/cgroup").unwrap(),
        outside
    );
    for name in ["first", "second"] {
        let proof: Value = serde_json::from_slice(
            &std::fs::read(root.path().join(format!("{name}.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(proof["max"], cap.to_string());
        assert_eq!(proof["swap"], "0");
        assert_eq!(proof["oom_group"], "1");
        assert!(
            !Path::new(&format!("/proc/{}", proof["pid"].as_u64().unwrap())).exists(),
            "allocation process survived cleanup"
        );
    }
    (root, stopped)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn aggregate_child_memory_limit_kills_group_and_proves_cleanup() {
    let (_, stopped) = run(40 * MIB, 64 * MIB, false).await;
    assert!(
        stopped["frame"]["result"]["Err"]["message"]
            .as_str()
            .unwrap()
            .contains("aggregate memory limit exceeded"),
        "{stopped}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn aggregate_child_memory_limit_allows_two_children_that_fit() {
    let (_, stopped) = run(12 * MIB, 96 * MIB, false).await;
    assert_eq!(stopped["frame"]["result"], json!({"Ok":null}), "{stopped}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn aggregate_memory_cleanup_reaps_setsid_grandchildren() {
    let (_, stopped) = run(12 * MIB, 96 * MIB, true).await;
    assert_eq!(stopped["frame"]["result"], json!({"Ok":null}), "{stopped}");
}
