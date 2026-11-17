use std::path::Path;
#[test]
fn all_bundled_drivers_and_declared_resource_files_are_readable_with_the_new_contracts() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../resources");
    for name in ["coder", "reviewer"] {
        let manifest = root
            .join("agent-drivers/local")
            .join(name)
            .join("latest/base.lua");
        let driver = agent_driver_core::read_driver(&manifest, &Default::default()).unwrap();
        mlua::Lua::new()
            .load(&driver.program)
            .into_function()
            .unwrap();
        let settings: serde_json::Value = toml::from_str(
            &std::fs::read_to_string(manifest.with_file_name("setting.toml")).unwrap(),
        )
        .unwrap();
        assert!(settings["initialization"]["custom"]
            .as_array()
            .unwrap()
            .is_empty());
        for locator in settings["initialization"]["default"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|entry| entry["command"].as_str())
            .filter(|line| line.starts_with("IMPORT "))
            .filter_map(|line| line.split_whitespace().nth(2))
        {
            let parsed = address::ParsedAddress::parse(locator).unwrap();
            match parsed.kind.as_str() {
                "tool" => {
                    let path = address::resolve_file(&root.join("tools"), locator).unwrap();
                    let tool = tool_resource_core::read_tool(&path).unwrap();
                    mlua::Lua::new()
                        .load(&tool.program)
                        .into_function()
                        .unwrap();
                }
                "prompt" => {
                    let path = address::resolve_file(&root.join("prompts"), locator).unwrap();
                    prompt_resource_core::read_prompt(&path, &Default::default()).unwrap();
                }
                "sandbox" => {
                    let path = address::resolve_file(&root.join("sandboxes"), locator).unwrap();
                    sandbox_resource_core::read_sandbox(&path, &Default::default()).unwrap();
                }
                other => panic!("unexpected dependency kind: {other}"),
            }
        }
    }
}

#[cfg(unix)]
mod mailbox_compaction {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use serde_json::{json, Value};
    use std::{
        io::{Read, Write},
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        time::Duration,
    };
    type Socket = tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >;
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            if self.0.try_wait().unwrap().is_none() {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
    }
    async fn request(
        socket: &mut Socket,
        kind: &str,
        id: &str,
        message: Value,
        seen: &mut Vec<Value>,
    ) -> Value {
        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                json!({"type":kind,"id":id,"message":message})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let frame = socket.next().await.unwrap().unwrap();
                if let tokio_tungstenite::tungstenite::Message::Text(text) = frame {
                    let value: Value = serde_json::from_str(&text).unwrap();
                    seen.push(value.clone());
                    if value["id"] == id {
                        assert_eq!(value["success"], true, "{value}");
                        return value;
                    }
                }
            }
        })
        .await
        .unwrap()
    }
    #[tokio::test]
    async fn task_send_triggers_shipped_driver_compaction_without_agent_wake_or_turn() {
        let resources = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../resources")
            .canonicalize()
            .unwrap();
        for (name, legacy_config) in [("coder", false), ("reviewer", false), ("coder", true)] {
            let home = tempfile::tempdir().unwrap();
            let data = home.path().join(".margatroid");
            std::fs::create_dir(&data).unwrap();
            for directory in ["tools", "prompts", "sandboxes", "skills"] {
                std::os::unix::fs::symlink(resources.join(directory), data.join(directory))
                    .unwrap();
            }
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let endpoint = listener.local_addr().unwrap();
            let stop = Arc::new(AtomicBool::new(false));
            let stopped = stop.clone();
            let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let observed_attempts = attempts.clone();
            let model = std::thread::spawn(move || {
                while !stopped.load(Ordering::Acquire) {
                    let (mut socket, _) = match listener.accept() {
                        Ok(value) => value,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(1));
                            continue;
                        }
                        Err(error) => panic!("{error}"),
                    };
                    socket
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut bytes = Vec::new();
                    let mut byte = [0];
                    while !bytes.ends_with(b"\r\n\r\n") {
                        socket.read_exact(&mut byte).unwrap();
                        bytes.push(byte[0]);
                    }
                    let header = String::from_utf8(bytes).unwrap();
                    let length: usize = header
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|value| value.trim().parse().unwrap())
                        })
                        .unwrap();
                    let mut body = vec![0; length];
                    socket.read_exact(&mut body).unwrap();
                    let body: Value = serde_json::from_slice(&body).unwrap();
                    assert!(body["messages"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|message| message["content"] == "old question"));
                    if observed_attempts.fetch_add(1, Ordering::AcqRel) == 0 {
                        socket.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").unwrap();
                        continue;
                    }
                    let trace="data: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"manual summary\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
                    write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",trace.len(),trace).unwrap();
                }
            });
            let driver = data.join(format!("agent-drivers/local/{name}/latest"));
            std::fs::create_dir_all(&driver).unwrap();
            std::fs::copy(
                resources.join(format!("agent-drivers/local/{name}/latest/base.lua")),
                driver.join("base.lua"),
            )
            .unwrap();
            std::fs::write(driver.join("setting.toml"),format!("sandbox=[]\n[inference]\nmodel='offline'\n[compaction]\nthreshold=0.8\nkeep_messages=2\n[initialization]{}",std::fs::read_to_string(resources.join(format!("agent-drivers/local/{name}/latest/setting.toml"))).unwrap().split_once("[initialization]").unwrap().1)).unwrap();

            let settings_file = driver.join("setting.toml");
            let text=std::fs::read_to_string(&settings_file).unwrap().replace("custom = []","custom = [{command='IMPORT TOOL tool:local/bash-exec:latest AS init_exec',arguments=[]}, {command='CREATE TEMP BLOCK user_setup (messages)',arguments=[]}, {command='INJECT soul TO user_setup.messages',arguments=[]}, {command='EFFECT tool_call ?', arguments=[[{id='init-tool',tool_name='init_exec',arguments='{\"cmd\":\"printf initialized > initialization-result.txt\"}'}]]}]");
            std::fs::write(settings_file, text).unwrap();
            if legacy_config {
                let file = data.join(format!("workspaces/probe/agents/{name}/setting.toml"));
                std::fs::create_dir_all(file.parent().unwrap()).unwrap();
                std::fs::write(file,"legacy_marker=true\nsandbox=[]\n[inference]\nmodel='offline'\n[compaction]\nthreshold=0.8\nkeep_messages=2\n").unwrap();
            }
            let port = std::net::TcpListener::bind("127.0.0.1:0")
                .unwrap()
                .local_addr()
                .unwrap()
                .port();
            std::fs::write(
                data.join("config.toml"),
                format!("[server]\nbind='127.0.0.1:{port}'\n"),
            )
            .unwrap();
            std::fs::write(data.join("models.toml"),format!("[[models]]\nid='offline'\nmodel='local'\nprovider='openai'\napi_type='chat_completions'\nbase_url='http://{endpoint}/v1'\napi_key='test'\ncontext_window='128k'\n")).unwrap();
            let definition = data.join("workspaces/probe/workspace.yaml");
            std::fs::create_dir_all(definition.parent().unwrap()).unwrap();
            std::fs::write(&definition,format!("name: probe\nproject_root: {}\nmembers:\n- name: {name}\n  driver: driver:local/{name}:latest\n  home_root: {}\n",home.path().display(),home.path().display())).unwrap();
            let mut child = Child(
                std::process::Command::new(
                    std::env::var_os("MARGATROID_DRIVER_TEST_BINARY")
                        .unwrap_or_else(|| env!("CARGO_BIN_EXE_margatroid").into()),
                )
                .env("HOME", home.path())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap(),
            );
            let mut socket = tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    if let Ok((socket, _)) =
                        tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws")).await
                    {
                        break socket;
                    }
                    assert!(child.0.try_wait().unwrap().is_none());
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            let mut seen = Vec::new();
            request(
                &mut socket,
                "connection.register",
                "register",
                json!({"client_type":"webui","name":"mailbox-test"}),
                &mut seen,
            )
            .await;
            request(
                &mut socket,
                "workspace.start",
                "start",
                json!({"path":definition}),
                &mut seen,
            )
            .await;
            let address = format!("agent:probe/{name}:latest");
            let persistent_setup=request(&mut socket,"mcl.execute","persistent-import",json!({"address":address,"command":"IMPORT TOOL tool:local/bash:latest AS restart_probe","arguments":[]}),&mut seen).await;
            assert_eq!(persistent_setup["success"], true);
            let started=request(&mut socket,"mcl.execute","persistent-start",json!({"address":address,"command":"EFFECT tool_call ?","arguments":[[{"id":"persistent-before-restart","tool_name":"restart_probe","arguments":"{}"}]]}),&mut seen).await;
            assert_eq!(started["success"], true);
            let tasks = request(
                &mut socket,
                "agent.task.req",
                "before-restart-tasks",
                json!({"address":address}),
                &mut seen,
            )
            .await;
            let task = tasks["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .find(|task| task["call_id"] == "persistent-before-restart")
                .expect("returned persistent task");
            assert_eq!(task["returned"], true);
            let old_task = task["task_id"].clone();
            request(
                &mut socket,
                "workspace.restart",
                "restart-with-returned-task",
                json!({"path":definition}),
                &mut seen,
            )
            .await;
            let after = request(
                &mut socket,
                "agent.task.req",
                "after-restart-tasks",
                json!({"address":address}),
                &mut seen,
            )
            .await;
            assert!(after["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .all(|task| task["task_id"] != old_task));
            let config = request(
                &mut socket,
                "mcl.execute",
                "config",
                json!({"address":address,"command":"CONFIG GET","arguments":[]}),
                &mut seen,
            )
            .await;
            if legacy_config {
                assert_eq!(config["data"]["value"]["legacy_marker"], true);
                assert!(config["data"]["value"]["initialization"].is_null());
            }
            let custom = request(
                &mut socket,
                "mcl.execute",
                "custom-initialized",
                json!({"address":address,"command":"GET user_setup.messages","arguments":[]}),
                &mut seen,
            )
            .await;
            assert!(!custom["data"]["value"].as_array().unwrap().is_empty());
            assert_eq!(
                std::fs::read_to_string(home.path().join("initialization-result.txt")).unwrap(),
                "initialized"
            );
            let target = config["data"]["value"]["driver_task"].clone();
            assert!(target["agent"].as_str().unwrap().starts_with("execution:"));
            assert!(!target["task_id"].as_str().unwrap().is_empty());
            let original = json!([{"type":"user","content":"old question"},{"type":"assistant","content":"old answer"},{"type":"user","content":"recent question"},{"type":"assistant","content":"recent answer"}]);
            request(&mut socket,"mcl.execute","context",json!({"address":address,"command":"INJECT ? TO session.conversation","arguments":[original]}),&mut seen).await;
            let baseline = request(
                &mut socket,
                "mcl.execute",
                "baseline-context",
                json!({"address":address,"command":"GET session.conversation","arguments":[]}),
                &mut seen,
            )
            .await;
            request(&mut socket,"mcl.execute","trigger",json!({"address":address,"command":"EFFECT tool_call ?","arguments":[[{"id":"compact-send","tool_name":"task_send","arguments":json!({"task_id":target,"value":{"type":"compact"}}).to_string()}]]}),&mut seen).await;
            let failed = tokio::time::timeout(Duration::from_secs(15), async {
                loop {
                    if let Some(value) = seen.iter().find(|value| {
                        value["type"] == "agent.message.full.sync"
                            && value["message"]["type"] == "error"
                            && value["generation"].as_str().is_some()
                    }) {
                        break value.clone();
                    }
                    let frame = socket.next().await.unwrap().unwrap();
                    if let tokio_tungstenite::tungstenite::Message::Text(text) = frame {
                        seen.push(serde_json::from_str::<Value>(&text).unwrap());
                    }
                }
            })
            .await
            .unwrap();
            assert!(failed["message"]["message"]
                .as_str()
                .unwrap()
                .contains("503"));
            tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    let history = request(
                        &mut socket,
                        "agent.history.req",
                        "failed-history",
                        json!({"address":address}),
                        &mut seen,
                    )
                    .await;
                    if history["records"].as_array().unwrap().iter().any(|row| {
                        row["payload"]["type"] == "error"
                            && row["payload"]["request_id"] == failed["message"]["request_id"]
                    }) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
            let unchanged = request(
                &mut socket,
                "mcl.execute",
                "failed-context",
                json!({"address":address,"command":"GET session.conversation","arguments":[]}),
                &mut seen,
            )
            .await;
            assert_eq!(unchanged["data"]["value"], baseline["data"]["value"]);
            let snapshots=request(&mut socket,"mcl.execute","failed-snapshots",json!({"address":address,"command":"EFFECT block_snapshots ?","arguments":["session"]}),&mut seen).await;
            assert!(snapshots["data"]["value"]["snapshots"]
                .as_array()
                .unwrap()
                .is_empty());
            let agents = request(
                &mut socket,
                "agent.req",
                "still-running",
                json!({}),
                &mut seen,
            )
            .await;
            assert_eq!(agents["agents"][0]["status"], "running");
            assert_eq!(
                attempts.load(Ordering::Acquire),
                1,
                "failed compaction must not retry itself"
            );
            request(&mut socket,"mcl.execute","second-trigger",json!({"address":address,"command":"EFFECT tool_call ?","arguments":[[{"id":"compact-send-again","tool_name":"task_send","arguments":json!({"task_id":target,"value":{"type":"compact"}}).to_string()}]]}),&mut seen).await;
            let compact = tokio::time::timeout(Duration::from_secs(15), async {
                loop {
                    if let Some(value) = seen.iter().find(|value| {
                        value["type"] == "agent.message.full.sync"
                            && value["message"]["type"] == "compact"
                    }) {
                        break value.clone();
                    }
                    let frame = socket.next().await.unwrap().unwrap();
                    if let tokio_tungstenite::tungstenite::Message::Text(text) = frame {
                        seen.push(serde_json::from_str::<Value>(&text).unwrap());
                    }
                }
            })
            .await
            .unwrap();
            assert_eq!(compact["message"]["content"], "manual summary");
            assert!(compact["message"]["sequence"].as_u64().unwrap() > 0);
            assert!(compact["generation"].is_null());
            tokio::time::timeout(Duration::from_secs(10),async{loop{let context=request(&mut socket,"mcl.execute","result-context",json!({"address":address,"command":"GET session.conversation","arguments":[]}),&mut seen).await;if context["data"]["value"][0]["content"]=="Conversation checkpoint:\nmanual summary"{break;}tokio::time::sleep(Duration::from_millis(5)).await;}}).await.unwrap();
            let state = request(
                &mut socket,
                "agent.turn.req",
                "turn",
                json!({"address":address}),
                &mut seen,
            )
            .await;
            assert!(state["state"]["turn"].is_null());
            request(&mut socket,"mcl.execute","normal-inject",json!({"address":address,"command":"INJECT ? TO entry.pending[-1]","arguments":[[{"type":"inject","messages":[{"type":"user","content":"after-mailbox"}]}]]}),&mut seen).await;
            request(
                &mut socket,
                "mcl.execute",
                "normal-wake",
                json!({"address":address,"command":"EFFECT wake","arguments":[]}),
                &mut seen,
            )
            .await;
            tokio::time::timeout(Duration::from_secs(10),async{loop{let context=request(&mut socket,"mcl.execute","post-mailbox-context",json!({"address":address,"command":"GET session.conversation","arguments":[]}),&mut seen).await;if context["data"]["value"].as_array().unwrap().last().unwrap()["content"]=="after-mailbox"{break;}tokio::time::sleep(Duration::from_millis(5)).await;}}).await.unwrap();
            request(
                &mut socket,
                "workspace.stop",
                "stop",
                json!({"path":definition}),
                &mut seen,
            )
            .await;
            assert_eq!(
                seen.iter()
                    .filter(|value| value["type"] == "agent.message.full.sync"
                        && value["message"]["type"] == "error"
                        && value["message"]["request_id"] == failed["message"]["request_id"])
                    .count(),
                1,
                "one failed inference must deliver exactly one error full"
            );
            socket.close(None).await.unwrap();
            child.0.kill().unwrap();
            child.0.wait().unwrap();
            stop.store(true, Ordering::Release);
            model.join().unwrap();
        }
    }
    #[tokio::test]
    async fn model_file_api_reads_updates_and_syncs_without_starting_a_workspace() {
        let home = tempfile::tempdir().unwrap();
        let data = home.path().join(".margatroid");
        std::fs::create_dir(&data).unwrap();
        for directory in [
            "workspaces",
            "tools",
            "prompts",
            "sandboxes",
            "skills",
            "agent-drivers",
        ] {
            std::fs::create_dir(data.join(directory)).unwrap();
        }
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        std::fs::write(
            data.join("config.toml"),
            format!("[server]\nbind='127.0.0.1:{port}'\n"),
        )
        .unwrap();
        let original="[[models]]\nid='route'\nmodel='first-model'\nprovider='openai'\napi_type='chat_completions'\nbase_url='http://127.0.0.1:1/v1'\napi_key='test-key'\ncontext_window='128k'\n";
        let path = data.join("models.toml");
        std::fs::write(&path, original).unwrap();
        let mut child = Child(
            std::process::Command::new(
                std::env::var_os("MARGATROID_DRIVER_TEST_BINARY")
                    .unwrap_or_else(|| env!("CARGO_BIN_EXE_margatroid").into()),
            )
            .env("HOME", home.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
        );
        let mut socket = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Ok((socket, _)) =
                    tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws")).await
                {
                    break socket;
                }
                assert!(child.0.try_wait().unwrap().is_none());
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let mut seen = Vec::new();
        request(
            &mut socket,
            "connection.register",
            "model-register",
            json!({"client_type":"webui","name":"model-editor"}),
            &mut seen,
        )
        .await;
        let workspaces = request(
            &mut socket,
            "workspace.req",
            "empty-workspaces",
            json!({}),
            &mut seen,
        )
        .await;
        assert!(workspaces["workspaces"].as_array().unwrap().is_empty());
        let response = request(
            &mut socket,
            "model.config.req",
            "read-file",
            json!({}),
            &mut seen,
        )
        .await;
        assert_eq!(response["content"], original);
        let updated = original.replace("first-model", "updated-model");
        request(
            &mut socket,
            "model.config.update",
            "update-file",
            json!({"content":updated}),
            &mut seen,
        )
        .await;
        assert_eq!(std::fs::read_to_string(&path).unwrap(), updated);
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if seen.iter().any(|value| {
                    value["type"] == "model.config.sync" && value["content"] == updated
                }) {
                    break;
                }
                let frame = socket.next().await.unwrap().unwrap();
                if let tokio_tungstenite::tungstenite::Message::Text(text) = frame {
                    seen.push(serde_json::from_str::<Value>(&text).unwrap());
                }
            }
        })
        .await
        .unwrap();
        let external = updated.replace("updated-model", "external-model");
        let staging = data.join("models-next.toml");
        std::fs::write(&staging, &external).unwrap();
        std::fs::rename(staging, &path).unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let frame = socket.next().await.unwrap().unwrap();
                if let tokio_tungstenite::tungstenite::Message::Text(text) = frame {
                    let value: Value = serde_json::from_str(&text).unwrap();
                    if value["type"] == "model.config.sync" && value["content"] == external {
                        break;
                    }
                }
            }
        })
        .await
        .unwrap();
        socket.close(None).await.unwrap();
        child.0.kill().unwrap();
        child.0.wait().unwrap();
    }
}
