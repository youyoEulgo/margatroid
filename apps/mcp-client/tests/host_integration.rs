//! This test executable also serves as its own tool runner. Cargo's libtest
//! harness is disabled so supervisor/worker protocol stdout stays untouched.
#[cfg(target_os = "linux")]
mod linux {
    use agent_core::{Agent, AgentState};
    use app_runtime_plugin::RuntimePlugin;
    use async_runtime_plugin::AsyncRuntimePlugin;
    use closure_plugin::{AppClosureExt, ClosurePlugin};
    use core_plugin::{App, Entity};
    use serde_json::{Value, json};
    use std::{
        collections::BTreeSet,
        path::PathBuf,
        sync::{Arc, Mutex},
        time::{Duration, Instant},
    };
    use tokio::sync::oneshot;
    use tool_core::{
        InitToolTasks, ToolError, ToolErrorKind, ToolExecuteRequest, ToolExecuteResponse, ToolTasks,
    };
    use tool_plugin::ToolPlugin;
    use tool_resource_core::{ToolEntry, ToolReadLimits, Tools, read_tool};
    use workspace_core::{Workspace, WorkspaceState};

    type Reply = oneshot::Receiver<Result<ToolExecuteResponse, ToolError>>;
    const DEADLINE: Duration = Duration::from_secs(30);
    struct Fixture {
        app: App,
        agent: Entity,
        root: tempfile::TempDir,
        resource: PathBuf,
        trace: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let resource = root.path().join("mcp-register/latest");
            std::fs::create_dir_all(resource.join("bin")).unwrap();
            let bundled = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../resources/tools/local/mcp-register/latest");
            for name in ["tool.toml", "input.schema.json", "main.lua"] {
                std::fs::copy(bundled.join(name), resource.join(name)).unwrap();
            }
            std::fs::copy(
                env!("CARGO_BIN_EXE_mcp-client"),
                resource.join("bin/mcp-client"),
            )
            .unwrap();
            let imported =
                read_tool(&resource.join("tool.toml"), &ToolReadLimits::default()).unwrap();
            let mut tools = Tools::default();
            tools
                .insert(
                    "mcp_register".into(),
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
                .add_plugin(ToolPlugin::new().with_runner_path(std::env::current_exe().unwrap()));
            let workspace = app.world_mut().spawn();
            app.world_mut().insert_component(
                workspace,
                Workspace {
                    name: "mcp-integration".into(),
                    definition_path: root.path().join("workspace.yaml"),
                    project_root: root.path().to_owned(),
                    state: WorkspaceState::Running,
                    agents: BTreeSet::new(),
                },
            );
            let agent = app.world_mut().spawn();
            app.world_mut().insert_component(
                agent,
                Agent {
                    name: "mcp-integration".into(),
                    workspace,
                    state: AgentState::Working,
                    turn: Some("integration-turn".into()),
                },
            );
            app.world_mut().insert_component(agent, tools);
            let (sender, mut reply) = oneshot::channel();
            app.world().emit_event(InitToolTasks {
                agent,
                reply: Arc::new(Mutex::new(Some(sender))),
            });
            app.tick();
            assert!(reply.try_recv().unwrap().is_ok());
            let trace = root.path().join("remote-trace.jsonl");
            Self {
                app,
                agent,
                root,
                resource,
                trace,
            }
        }
        fn execute(&mut self, id: &str, alias: &str, arguments: Value) -> Reply {
            let (sender, reply) = oneshot::channel();
            self.app.world().emit_event(ToolExecuteRequest {
                id: id.into(),
                agent: self.agent,
                turn: "integration-turn".into(),
                alias: alias.into(),
                call_id: format!("call-{id}"),
                arguments,
                reply: Arc::new(Mutex::new(Some(sender))),
            });
            self.app.tick();
            self.app.tick();
            reply
        }
        fn response(&mut self, mut reply: Reply) -> Result<ToolExecuteResponse, ToolError> {
            let deadline = Instant::now() + DEADLINE;
            loop {
                self.app.tick();
                match reply.try_recv() {
                    Ok(response) => return response,
                    Err(oneshot::error::TryRecvError::Empty) => {}
                    Err(error) => panic!("tool reply closed: {error}"),
                }
                assert!(Instant::now() < deadline, "tool response timed out");
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        fn until(&mut self, mut condition: impl FnMut(&Self) -> bool) {
            let deadline = Instant::now() + DEADLINE;
            while !condition(self) {
                self.app.tick();
                assert!(
                    Instant::now() < deadline,
                    "host integration condition timed out"
                );
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        fn tasks(&self) -> ToolTasks {
            self.app
                .world()
                .get_component::<ToolTasks>(self.agent)
                .unwrap()
                .clone()
        }
        fn trace(&self) -> Vec<Value> {
            std::fs::read_to_string(&self.trace)
                .unwrap_or_default()
                .lines()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect()
        }
        fn finish(&mut self, id: &str) {
            let tasks = self.tasks();
            let id = id.to_owned();
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let (sender, reply) = std::sync::mpsc::channel();
            // Host callbacks need App ticks while the async cleanup future runs.
            let thread = std::thread::spawn(move || {
                let _ = sender.send(runtime.block_on(tasks.finish(&id)));
            });
            let mut result = None;
            self.until(|_| {
                result = reply.try_recv().ok();
                result.is_some()
            });
            result.unwrap().unwrap();
            thread.join().unwrap();
        }
    }
    pub fn large_catalog() {
        let mut f = Fixture::new();
        let server = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/server.py");
        let registration = f.execute(
            "catalog-owner",
            "mcp_register",
            json!({
                "alias_prefix":"large_", "server": {
                    "transport":"stdio", "command":"python3", "args":[server],
                    "env":{"MCP_FIXTURE_TRACE":f.trace,"MCP_FIXTURE_LARGE_CATALOG":"1"},
                }
            }),
        );
        let response = f.response(registration).unwrap();
        assert!(
            !response.failed,
            "large catalog registration failed: {}",
            response.content
        );
        let registered: Value = serde_json::from_str(&response.content).unwrap();
        assert_eq!(registered["task_id"], "catalog-owner");
        let aliases = registered["aliases"].as_array().unwrap();
        assert_eq!(aliases.len(), 1024);
        for (index, alias) in aliases.iter().enumerate() {
            assert_eq!(alias, &json!(format!("large_catalog_{index:04}")));
        }
        let tools = f.app.world().get_component::<Tools>(f.agent).unwrap();
        assert_eq!(tools.iter().count(), 1025);
        let definitions: Vec<_> = tools
            .iter()
            .filter(|(_, tool)| tool.owner_task_id.as_deref() == Some("catalog-owner"))
            .map(|(alias, tool)| {
                json!({
                    "alias":alias, "description":tool.description,
                    "input_schema":tool.input_schema, "binding":tool.binding,
                })
            })
            .collect();
        assert_eq!(definitions.len(), 1024);
        let definition_bytes = serde_json::to_vec(&definitions).unwrap().len();
        assert!((8 * 1024 * 1024..=12 * 1024 * 1024).contains(&definition_bytes));
        for index in [0, 1023] {
            let marker = format!("{index:04}");
            let tool = tools.get(&format!("large_catalog_{marker}")).unwrap();
            assert_eq!(tool.owner_task_id.as_deref(), Some("catalog-owner"));
            assert_eq!(tool.path, f.resource.join("tool.toml"));
            assert_eq!(
                tool.binding,
                Some(json!({"remote_tool":format!("catalog_{marker}")}))
            );
            assert_eq!(
                tool.description,
                format!(
                    "catalog-description-{marker}:{}:description-tail-{marker}",
                    "x".repeat(8192)
                )
            );
            assert_eq!(
                tool.input_schema,
                json!({
                    "type":"object", "title":format!("catalog-tool-{marker}"),
                    "properties": {
                        "text":{"type":"string","description":format!("schema-property-tail-{marker}")},
                        "reverse":{"type":"boolean"}, "pid":{"type":"boolean"},
                        "hang":{"type":"boolean"},
                    },
                    "additionalProperties":true, "$comment":format!("catalog-schema-tail-{marker}"),
                })
            );
        }
        let catalog_trace = f
            .trace()
            .into_iter()
            .find(|v| v.get("catalog_tools").is_some())
            .unwrap();
        assert_eq!(catalog_trace["catalog_tools"], 1024);
        let catalog_bytes = catalog_trace["catalog_json_bytes"].as_u64().unwrap();
        assert!((8 * 1024 * 1024..=12 * 1024 * 1024).contains(&catalog_bytes));
        assert!(f.tasks().get("catalog-owner").is_ok());

        let first = f.execute(
            "catalog-first",
            "large_catalog_0000",
            json!({"reverse":true,"text":"catalog-first-value"}),
        );
        f.until(|f| {
            f.trace()
                .iter()
                .any(|v| v["arguments"]["text"] == "catalog-first-value")
        });
        let last = f.execute(
            "catalog-last",
            "large_catalog_1023",
            json!({"reverse":true,"text":"catalog-last-value"}),
        );
        let last_response = f.response(last).unwrap();
        let first_response = f.response(first).unwrap();
        assert_eq!(last_response.alias, "large_catalog_1023");
        assert_eq!(last_response.call_id, "call-catalog-last");
        assert_eq!(last_response.content, "catalog-last-value");
        assert_eq!(first_response.alias, "large_catalog_0000");
        assert_eq!(first_response.call_id, "call-catalog-first");
        assert_eq!(first_response.content, "catalog-first-value");
        assert!(!first_response.failed && !last_response.failed);
        let emitted: Vec<_> = f
            .trace()
            .into_iter()
            .filter_map(|v| {
                v["emitted"]["result"]["content"][0]["text"]
                    .as_str()
                    .map(str::to_owned)
            })
            .collect();
        assert_eq!(emitted, ["catalog-last-value", "catalog-first-value"]);
        f.until(|f| {
            f.tasks().get("catalog-first").is_err() && f.tasks().get("catalog-last").is_err()
        });

        let pids = f.execute("catalog-pids", "large_catalog_0000", json!({"pid":true}));
        let response = f.response(pids).unwrap();
        assert!(!response.failed, "{}", response.content);
        let pids: Value = serde_json::from_str(&response.content).unwrap();
        let waiting = f.execute(
            "catalog-waiting",
            "large_catalog_1023",
            json!({"hang":true}),
        );
        f.until(|f| f.trace().iter().any(|v| v["arguments"]["hang"] == true));
        f.finish("catalog-owner");
        assert_eq!(
            f.response(waiting).unwrap_err().kind,
            ToolErrorKind::OwnerGone
        );
        f.until(|f| f.tasks().list().is_empty());
        let tools = f.app.world().get_component::<Tools>(f.agent).unwrap();
        assert_eq!(tools.iter().count(), 1);
        assert!(tools.get("mcp_register").is_some());
        for index in 0..1024 {
            assert!(tools.get(&format!("large_catalog_{index:04}")).is_none());
        }
        for key in ["host", "pid", "child"] {
            let pid = pids[key].as_u64().unwrap();
            assert!(
                !std::path::Path::new(&format!("/proc/{pid}")).exists(),
                "{key} process {pid} survived large catalog owner cleanup"
            );
        }
        println!(
            "host_integration large_catalog: tools=1024 host_records=1025 catalog_json_bytes={catalog_bytes} definition_json_bytes={definition_bytes}; full descriptions/schemas, reverse concurrent calls, all aliases/tasks/processes cleanup passed"
        );
    }
    pub fn result_formatting() {
        let mut f = Fixture::new();
        let server = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/server.py");
        let registration = f.execute(
            "format-owner",
            "mcp_register",
            json!({
                "alias_prefix":"format_", "server": {
                    "transport":"stdio", "command":"python3", "args":[server],
                }
            }),
        );
        assert!(!f.response(registration).unwrap().failed);
        let structured = json!({"result":{"items":[{"value":1,"empty":{},"array":[],"missing":null}],"total":1}});
        let mirror = r#"{ "result": { "total": 1, "items": [{ "missing": null, "array": [], "empty": {}, "value": 1 }] } }"#;
        for (index, texts) in [vec![mirror], vec![mirror, mirror]].into_iter().enumerate() {
            let blocks: Vec<_> = texts
                .into_iter()
                .map(|text| json!({"type":"text","text":text}))
                .collect();
            let reply = f.execute(
                &format!("mirror-{index}"),
                "format_echo",
                json!({"result_case":{"content":blocks,"structuredContent":structured}}),
            );
            let response = f.response(reply).unwrap();
            assert!(!response.failed);
            assert_eq!(
                serde_json::from_str::<Value>(&response.content).unwrap(),
                structured
            );
            assert_eq!(response.content.lines().count(), 1);
        }
        let original = "unchanged text\nwith newlines";
        let reply = f.execute(
            "text-only",
            "format_echo",
            json!({"result_case":{"content":[{"type":"text","text":original}]}}),
        );
        assert_eq!(f.response(reply).unwrap().content, original);
        // These pairs must remain distinct: array order, scalar type, null,
        // absent keys, and empty object/array kinds all matter.
        for (index, (text, value)) in [
            ("query completed", json!({"value":1})),
            (r#"{"value":2}"#, json!({"value":1})),
            (r#"{"value":"1"}"#, json!({"value":1})),
            (r#"{"value":false}"#, json!({"value":0})),
            (r#"{"value":[2,1]}"#, json!({"value":[1,2]})),
            (r#"{"value":[]}"#, json!({"value":{}})),
            (r#"{}"#, json!({"value":null})),
            ("not valid JSON {", json!({"value":1})),
        ]
        .into_iter()
        .enumerate()
        {
            let reply = f.execute(&format!("different-{index}"), "format_echo", json!({"result_case":{"content":[{"type":"text","text":text}],"structuredContent":value}}));
            let response = f.response(reply).unwrap();
            let (first, second) = response.content.split_once('\n').unwrap();
            assert_eq!(first, text);
            assert_eq!(serde_json::from_str::<Value>(second).unwrap(), value);
        }
        let image = json!({"type":"image","mimeType":"image/png","data":"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jWZkAAAAASUVORK5CYII="});
        let audio = json!({"type":"audio","mimeType":"audio/wav","data":"YXVkaW8="});
        let resource = json!({"type":"resource_link","uri":"example://resource","name":"extra"});
        let reply = f.execute("mixed-content", "format_echo", json!({"result_case":{
            "content":[{"type":"text","text":"independent explanation"},{"type":"text","text":mirror},image,audio,resource],
            "structuredContent":structured
        }}));
        let response = f.response(reply).unwrap();
        assert!(!response.failed);
        let lines: Vec<_> = response.content.lines().collect();
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0], "independent explanation");
        assert_eq!(serde_json::from_str::<Value>(lines[1]).unwrap(), audio);
        assert_eq!(serde_json::from_str::<Value>(lines[2]).unwrap(), resource);
        assert_eq!(serde_json::from_str::<Value>(lines[3]).unwrap(), structured);
        assert_eq!(response.images.len(), 1);
        assert_eq!(response.images[0].media_type, "image/png");
        assert_eq!(response.images[0].data, image["data"]);
        let reply = f.execute("failed-mirror", "format_echo", json!({"result_case":{
            "content":[{"type":"text","text":mirror}],"structuredContent":structured,"isError":true
        }}));
        let response = f.response(reply).unwrap();
        assert!(response.failed);
        assert_eq!(
            serde_json::from_str::<Value>(&response.content).unwrap(),
            structured
        );
        f.finish("format-owner");
        f.until(|f| f.tasks().list().is_empty());
        println!(
            "host_integration formatting: semantic mirrors emitted once; distinct text, JSON kinds, mixed media and failures preserved"
        );
    }

    pub fn run() {
        let mut f = Fixture::new();
        let server = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/server.py");
        let registration = f.execute(
            "owner",
            "mcp_register",
            json!({
                "alias_prefix":"e2e_", "server": {
                    "transport":"stdio", "command":"python3", "args":[server],
                    "env":{"MCP_FIXTURE_TRACE":f.trace,"MCP_FIXTURE_ENV":"end-to-end"},
                }
            }),
        );
        let response = f.response(registration).unwrap();
        assert!(
            !response.failed,
            "registration failed: {}",
            response.content
        );
        let registered: Value = serde_json::from_str(&response.content).unwrap();
        assert_eq!(registered["task_id"], "owner");
        assert_eq!(registered["aliases"], json!(["e2e_echo", "e2e_other"]));
        let tools = f.app.world().get_component::<Tools>(f.agent).unwrap();
        for (alias, remote) in [("e2e_echo", "echo"), ("e2e_other", "other")] {
            let tool = tools.get(alias).unwrap();
            assert_eq!(tool.owner_task_id.as_deref(), Some("owner"));
            assert_eq!(tool.binding, Some(json!({"remote_tool":remote})));
            assert_eq!(tool.path, f.resource.join("tool.toml"));
            assert!(
                !tool.description.is_empty(),
                "description-less remote tool needs a host-valid fallback"
            );
        }
        assert!(f.tasks().get("owner").is_ok());

        // Wait until the first request actually reaches the remote process before
        // sending the second: the remote refuses to answer until both are in flight.
        let first = f.execute(
            "first",
            "e2e_echo",
            json!({"reverse":true,"text":"first-value"}),
        );
        f.until(|f| {
            f.trace()
                .iter()
                .any(|v| v["arguments"]["text"] == "first-value")
        });
        let second = f.execute(
            "second",
            "e2e_other",
            json!({"reverse":true,"text":"second-value"}),
        );
        let second_response = f.response(second).unwrap();
        let first_response = f.response(first).unwrap();
        assert_eq!(second_response.alias, "e2e_other");
        assert_eq!(second_response.call_id, "call-second");
        assert_eq!(second_response.content, "second-value");
        assert_eq!(first_response.content, "first-value");
        assert!(!first_response.failed && !second_response.failed);
        let emitted: Vec<_> = f
            .trace()
            .into_iter()
            .filter_map(|v| {
                v["emitted"]["result"]["content"][0]["text"]
                    .as_str()
                    .map(str::to_owned)
            })
            .collect();
        assert_eq!(emitted, ["second-value", "first-value"]);
        f.until(|f| f.tasks().get("first").is_err() && f.tasks().get("second").is_err());

        let pids = f.execute("pids", "e2e_echo", json!({"pid":true}));
        let response = f.response(pids).unwrap();
        assert!(!response.failed, "{}", response.content);
        let pids: Value = serde_json::from_str(&response.content).unwrap();
        assert_eq!(pids["env"], "end-to-end");
        let waiting = f.execute("waiting", "e2e_other", json!({"hang":true}));
        f.until(|f| f.trace().iter().any(|v| v["arguments"]["hang"] == true));
        f.finish("owner");
        assert_eq!(
            f.response(waiting).unwrap_err().kind,
            ToolErrorKind::OwnerGone
        );
        f.until(|f| f.tasks().list().is_empty());
        let tools = f.app.world().get_component::<Tools>(f.agent).unwrap();
        assert!(tools.get("mcp_register").is_some());
        assert!(tools.get("e2e_echo").is_none() && tools.get("e2e_other").is_none());
        // The supervisor is a subreaper: final cleanup must reap, not leave zombies.
        for key in ["host", "pid", "child"] {
            let pid = pids[key].as_u64().unwrap();
            assert!(
                !std::path::Path::new(&format!("/proc/{pid}")).exists(),
                "{key} process {pid} survived owner cleanup"
            );
        }
        assert!(f.root.path().exists());
        println!(
            "host_integration: actual resource registration, reverse concurrent calls, owner cleanup passed"
        );
    }
}

fn main() {
    if std::env::args()
        .nth(1)
        .is_some_and(|arg| arg.starts_with("--margatroid-tool-"))
    {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        std::process::exit(runtime.block_on(tool_plugin::runner_main()));
    }
    #[cfg(target_os = "linux")]
    {
        linux::run();
        linux::large_catalog();
        linux::result_formatting();
    }
    #[cfg(not(target_os = "linux"))]
    println!("host_integration: Linux supervisor test skipped on this platform");
}
