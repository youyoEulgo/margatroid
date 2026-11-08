#![cfg(target_os = "linux")]

#[path = "../../../crates/margatroid/tool/plugin/tests/support/shell_client.rs"]
mod shell_client;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use tokio_tungstenite::{tungstenite::Message, MaybeTlsStream, WebSocketStream};

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
const LIMIT: Duration = Duration::from_secs(20);

// Include start time so failure cleanup can never signal an unrelated reused PID.
#[derive(Clone, Debug)]
struct Process {
    pid: u32,
    parent: u32,
    start: String,
    command: String,
}

fn process(pid: u32) -> Option<Process> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (_, fields) = stat.rsplit_once(") ")?;
    let fields: Vec<_> = fields.split_whitespace().collect();
    Some(Process {
        pid,
        parent: fields.get(1)?.parse().ok()?,
        start: fields.get(19)?.to_string(),
        command: std::fs::read(format!("/proc/{pid}/cmdline"))
            .ok()?
            .split(|byte| *byte == 0)
            .map(String::from_utf8_lossy)
            .collect::<Vec<_>>()
            .join(" "),
    })
}

fn descendants(root: u32) -> Vec<Process> {
    let mut all: BTreeMap<_, _> = std::fs::read_dir("/proc")
        .unwrap()
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_str()?.parse().ok())
        .filter_map(process)
        .map(|p| (p.pid, p))
        .collect();
    let mut found = vec![root];
    let mut result = Vec::new();
    while let Some(parent) = found.pop() {
        let children: Vec<_> = all
            .values()
            .filter(|p| p.parent == parent)
            .map(|p| p.pid)
            .collect();
        for pid in children {
            found.push(pid);
            result.push(all.remove(&pid).unwrap());
        }
    }
    result
}

fn alive(p: &Process) -> bool {
    process(p.pid).is_some_and(|current| current.start == p.start)
}

fn signal(pid: u32, name: &str) {
    assert!(
        Command::new("/bin/kill")
            .args([name, &pid.to_string()])
            .status()
            .unwrap()
            .success(),
        "could not send {name} to {pid}"
    );
}

struct Daemon {
    child: Child,
    owned: Vec<Process>,
    log: PathBuf,
}

impl Daemon {
    fn log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        // Give supervisors a chance to reap children even if a fixture assertion fails.
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = Command::new("/bin/kill")
                .args(["-TERM", &self.child.id().to_string()])
                .stderr(Stdio::null())
                .status();
            let deadline = Instant::now() + Duration::from_secs(2);
            while self.child.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        // Force cleanup only for recorded test-owned PIDs with matching start times.
        self.owned.extend(descendants(self.child.id()));
        for p in self.owned.iter().rev().filter(|p| alive(p)) {
            let _ = Command::new("/bin/kill")
                .args(["-KILL", &p.pid.to_string()])
                .stderr(Stdio::null())
                .status();
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn fixture(home: &Path, address: std::net::SocketAddr) -> PathBuf {
    let root = home.join(".margatroid");
    write(
        &root,
        "config.toml",
        &format!(
            "[server]\nbind='{address}'\n[outbound]\nlogs=['type:unmatched']\nbackend_state=['type:webui']\n"
        ),
    );
    write(
        &root,
        "models.toml",
        "[[models]]\nid='offline'\nmodel='unused'\nbase_url='http://127.0.0.1:9/v1'\napi_key='test-only'\nprovider='openai'\napi_type='chat_completions'\ncontext_window='128k'\n",
    );
    write(
        &root,
        "agent-drivers/local/shutdown/latest/driver.toml",
        "schema_version=1\ndependencies=['tool:local/shutdown:latest']\n[inference]\nmodel='offline'\n",
    );
    write(
        &root,
        "agent-drivers/local/shutdown/latest/settings.toml",
        "",
    );
    write(
        &root,
        "agent-drivers/local/shutdown/latest/base.lua",
        r#"
local function command(text)
    local result = margatroid.driver:mcl(margatroid.context.agent_info.entity, text)
    assert(not result.Error, 'shutdown fixture MCL failed')
end
command('IMPORT TOOL tool:local/shutdown:latest AS shutdown')
command('EFFECT ready')
while true do margatroid.driver:wait() end
"#,
    );
    write(
        &root,
        "tools/local/shutdown/latest/tool.toml",
        "schema_version=1\ndescription='test-owned persistent child process'\n",
    );
    write(
        &root,
        "tools/local/shutdown/latest/input.schema.json",
        r#"{"type":"object","properties":{"kind":{"type":"string"}},"required":["kind"]}"#,
    );
    write(
        &root,
        "tools/local/shutdown/latest/main.lua",
        r#"
function configure() return {timeout_ms=0} end
function execute(arguments, context)
    local child
    if margatroid.context.arguments.kind == 'terminal' then
        local client = dofile(margatroid.context.resource_dir..'/shell.lua')(margatroid.context)
        child = client.terminal()
        child:write('/bin/bash ./children.sh terminal\r')
        local output = ''
        while not string.find(output, 'CHILDREN_READY', 1, true) do
            local chunk = child:read()
            assert(not chunk.eof, 'fixture terminal exited')
            output = output .. chunk.text
        end
    else
        child = margatroid.process:start('/bin/bash', {'./children.sh','process'})
        local output = ''
        while not string.find(output, 'CHILDREN_READY', 1, true) do
            local chunk = child:read()
            assert(not chunk.eof, 'fixture child exited')
            output = output .. chunk.data
        end
    end
    margatroid.task:return_result({ready=true, kind=margatroid.context.arguments.kind})
    -- Returning a result leaves both the Lua task and the subprocess alive.
    while true do margatroid.task:receive() end
end
"#,
    );
    shell_client::install_shell(&root.join("tools/local/shutdown/latest"));
    let project = home.join("project");
    write(
        &project,
        "children.sh",
        r#"#!/bin/bash
set -eu
prefix="$1"
echo $$ > "$prefix-shell.pid"
sleep 600 &
echo $! > "$prefix-sleep.pid"
setsid /bin/sh -c 'echo $$ > "$1"; exec sleep 600' child "$prefix-detached.pid" &
while [ ! -s "$prefix-detached.pid" ]; do :; done
printf 'CHILDREN_READY\n'
wait
"#,
    );
    write(
        &root,
        "workspaces/shutdown/workspace.yaml",
        &format!(
            "name: shutdown\nproject_root: {}\nmembers:\n  - name: fixture\n    driver: driver:local/shutdown:latest\n    home_root: {}\n",
            project.display(),
            home.display()
        ),
    );
    root.join("workspaces/shutdown/workspace.yaml")
}

async fn request(socket: &mut Socket, kind: &str, id: &str, message: Value) -> Value {
    socket
        .send(Message::Text(
            json!({"type":kind,"id":id,"message":message})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    tokio::time::timeout(LIMIT, async {
        while let Some(frame) = socket.next().await {
            if let Message::Text(text) = frame.unwrap() {
                let reply: Value = serde_json::from_str(&text).unwrap();
                if reply["id"] == id {
                    return reply;
                }
            }
        }
        panic!("daemon disconnected while answering {kind}");
    })
    .await
    .expect("daemon request timed out")
}

async fn mcl(
    socket: &mut Socket,
    workspace: &Value,
    agent: &Value,
    id: &str,
    command: &str,
    arguments: Value,
) -> Value {
    let reply = request(
        socket,
        "mcl.command",
        id,
        json!({"workspace":workspace,"agent":agent,"command":command,"arguments":arguments}),
    )
    .await;
    assert_eq!(reply["type"], "mcl.command_result", "{reply}");
    assert!(reply["result"].get("Err").is_none(), "{reply}");
    reply["result"]["Ok"].clone()
}

async fn shutdown_case(name: &str, repeat: bool) {
    let home = tempfile::tempdir().unwrap();
    let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = reservation.local_addr().unwrap();
    let definition = fixture(home.path(), address);
    let log_path = home.path().join("daemon.log");
    let log = std::fs::File::create(&log_path).unwrap();
    drop(reservation);
    let child = Command::new(env!("CARGO_BIN_EXE_margatroid-daemon"))
        .env("HOME", home.path())
        .stdin(Stdio::null())
        // A file cannot fill a pipe and stall the production event loop.
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .spawn()
        .unwrap();
    let mut daemon = Daemon {
        child,
        owned: vec![],
        log: log_path,
    };
    let deadline = Instant::now() + LIMIT;
    let mut socket = loop {
        match tokio_tungstenite::connect_async(format!("ws://{address}/ws")).await {
            Ok((socket, _)) => break socket,
            Err(_) => {
                assert!(
                    daemon.child.try_wait().unwrap().is_none(),
                    "daemon exited at startup: {}",
                    daemon.log()
                );
                assert!(
                    Instant::now() < deadline,
                    "daemon never listened: {}",
                    daemon.log()
                );
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
    };
    let registered = request(
        &mut socket,
        "connection.register",
        "register",
        json!({"client_type":"cli","name":"shutdown-test"}),
    )
    .await;
    assert_eq!(registered["type"], "connection.registered", "{registered}");
    let started = request(
        &mut socket,
        "workspace.start",
        "start",
        json!({"path":definition}),
    )
    .await;
    assert_eq!(started["type"], "workspace.started", "{started}");
    assert!(
        started["workspace"]["member_failures"]
            .as_array()
            .is_none_or(|failures| failures.is_empty()),
        "{started}"
    );
    let workspace = &started["workspace"]["id"];
    let agent = &started["workspace"]["agents"][0];
    mcl(
        &mut socket,
        workspace,
        agent,
        "turn",
        "EFFECT start_turn",
        json!([]),
    )
    .await;
    let calls: Vec<_> = ["terminal", "process"].into_iter().map(|kind| json!({"id":kind,"tool_name":"shutdown","arguments":json!({"kind":kind}).to_string()})).collect();
    let returned = mcl(
        &mut socket,
        workspace,
        agent,
        "tools",
        "EFFECT tool_call ?",
        json!([[{"type":"assistant","tool_calls":calls}]]),
    )
    .await;
    let results = returned["Array"]["Message"]
        .as_array()
        .unwrap_or_else(|| panic!("tool results: {returned}"));
    assert_eq!(results.len(), 2, "{returned}");
    assert!(
        results
            .iter()
            .all(|r| r["failed"] == false
                && r["content"].as_str().is_some_and(|s| s.contains("ready"))),
        "{returned}"
    );
    let deadline = Instant::now() + LIMIT;
    let pids = loop {
        let pids: Option<Vec<u32>> = ["terminal", "process"]
            .into_iter()
            .flat_map(|kind| {
                ["shell", "sleep", "detached"].map(move |part| format!("{kind}-{part}.pid"))
            })
            .map(|name| {
                std::fs::read_to_string(home.path().join("project").join(name))
                    .ok()?
                    .trim()
                    .parse()
                    .ok()
            })
            .collect();
        if let Some(pids) = pids {
            break pids;
        }
        assert!(
            Instant::now() < deadline,
            "fixture children not ready: {}",
            daemon.log()
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    };
    daemon.owned = descendants(daemon.child.id());
    assert!(
        pids.iter()
            .all(|pid| daemon.owned.iter().any(|p| p.pid == *pid)),
        "fixture child missing from daemon tree: {:?}",
        daemon.owned
    );
    assert_eq!(
        daemon
            .owned
            .iter()
            .filter(|p| p.command.contains("--margatroid-tool-supervisor"))
            .count(),
        2,
        "{:?}",
        daemon.owned
    );
    assert_eq!(
        daemon
            .owned
            .iter()
            .filter(|p| p.command.contains("--margatroid-tool-worker"))
            .count(),
        2,
        "{:?}",
        daemon.owned
    );
    assert!(daemon.owned.iter().all(alive));
    signal(daemon.child.id(), name);
    if repeat {
        // Send while shutdown is pending; neither delivery may restore SIGINT's
        // default disposition and terminate the daemon before child reaping.
        tokio::time::sleep(Duration::from_millis(1)).await;
        signal(daemon.child.id(), "-INT");
    }
    let deadline = Instant::now() + LIMIT;
    let status = loop {
        if let Some(status) = daemon.child.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "shutdown timed out; children: {:?}; {}",
            daemon.owned.iter().filter(|p| alive(p)).collect::<Vec<_>>(),
            daemon.log()
        );
        tokio::time::sleep(Duration::from_millis(2)).await;
    };
    // Check immediately at exit: waiting afterwards would hide premature exit.
    let survivors: Vec<_> = daemon.owned.iter().filter(|p| alive(p)).collect();
    assert!(
        status.success(),
        "daemon exited with {status}: {}",
        daemon.log()
    );
    assert!(
        survivors.is_empty(),
        "daemon exited before reaping {survivors:?}: {}",
        daemon.log()
    );
    if repeat {
        assert!(
            daemon.log().contains("cleanup is still in progress"),
            "second SIGINT was not handled while cleanup was pending: {}",
            daemon.log()
        );
    }
    assert!(
        daemon.log().contains("shutdown complete"),
        "{}",
        daemon.log()
    );
}

#[tokio::test]
async fn sigint_waits_for_returned_persistent_tasks_and_all_descendants() {
    shutdown_case("-INT", false).await;
}

#[tokio::test]
async fn sigterm_waits_for_returned_persistent_tasks_and_all_descendants() {
    shutdown_case("-TERM", false).await;
}

#[tokio::test]
async fn repeated_sigint_does_not_exit_before_child_cleanup() {
    shutdown_case("-INT", true).await;
}
