#![cfg(target_os = "linux")]

#[path = "support/storage.rs"]
mod storage_fixture;

#[path = "support/shell_client.rs"]
mod shell_client;

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agent_core::AgentState;
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin};
use core_plugin::{App, Entity};
use serde_json::{json, Value};
use tokio::sync::oneshot;
use tool_core::{
    InitToolTasks, ToolConfiguration, ToolError, ToolErrorKind as Kind, ToolExecuteRequest,
    ToolExecuteResponse, ToolExecutionLimits, ToolTasks,
};
use tool_plugin::ToolPlugin;
use tool_resource_core::{read_tool, ToolEntry, Tools};
use workspace_core::WorkspaceState;

fn imported_entry(path: &std::path::Path) -> ToolEntry {
    let tool = read_tool(path).unwrap();
    ToolEntry {
        path: tool.origin,
        description: tool.description,
        input_schema: tool.input_schema,
        owner_task_id: None,
        binding: None,
    }
}

struct Fixture {
    app: App,
    agent: Entity,
    root: tempfile::TempDir,
    configuration: ToolConfiguration,
}

impl Fixture {
    fn new() -> Self {
        Self::with_limits(ToolExecutionLimits::default())
    }

    fn with_limits(limits: ToolExecutionLimits) -> Self {
        let root = tempfile::tempdir().unwrap();
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(AsyncRuntimePlugin)
            .add_plugin(ClosurePlugin)
            .add_closure_system(RuntimePlugin::UPDATE)
            .add_plugin(
                ToolPlugin::new(crate::test_binding())
                    .with_runner_path(env!("CARGO_BIN_EXE_tool_runner"))
                    .with_limits(limits),
            );
        let workspace = app.world_mut().spawn();
        app.world_mut().insert_component(
            workspace,
            workspace_core::Workspace::new(
                "test".into(),
                root.path().join("workspace.yaml"),
                root.path().to_owned(),
                WorkspaceState::Running,
                BTreeSet::new(),
                None,
                crate::test_workspace_context(),
            ),
        );
        let agent = app.world_mut().spawn();
        storage_fixture::mount(app.world_mut(), agent, root.path());
        app.world_mut().insert_component(
            agent,
            agent_core::Agent::new(
                "test".into(),
                workspace,
                AgentState::Working,
                Some("test-turn".into()),
                crate::test_agent_context(),
            ),
        );
        let (sender, mut reply) = oneshot::channel();
        app.world().emit_event(InitToolTasks {
            agent,
            reply: Arc::new(Mutex::new(Some(sender))),
        });
        app.tick();
        assert!(reply.try_recv().unwrap().is_ok());
        Self {
            app,
            agent,
            root,
            configuration: ToolConfiguration { timeout_ms: 30_000 },
        }
    }

    fn tool(&mut self, alias: &str, program: &str) {
        // Test programs declare their own lifetimes; bundled programs keep their
        // author-supplied configure function and are never overridden here.
        let program = if program.contains("function configure(") {
            program.to_owned()
        } else {
            format!(
                "function configure(arguments, context) return {{timeout_ms={}}} end\n{program}",
                self.configuration.timeout_ms,
            )
        };
        let directory = self.root.path().join(alias);
        std::fs::create_dir_all(&directory).unwrap();
        if alias == "bash" || alias == "bash-exec" || program.contains("/shell.lua") {
            shell_client::install_shell(&directory);
        }
        std::fs::write(
            directory.join("tool.toml"),
            "schema_version=1\ndescription='test'\n",
        )
        .unwrap();
        std::fs::write(directory.join("input.schema.json"), "{}").unwrap();
        std::fs::write(directory.join("main.lua"), program).unwrap();
        if !self.app.world().contains_component::<Tools>(self.agent) {
            self.app
                .world_mut()
                .insert_component(self.agent, Tools::new(crate::test_probe()));
        }
        self.app
            .world_mut()
            .get_component_mut::<Tools>(self.agent)
            .unwrap()
            .insert(alias.into(), imported_entry(&directory.join("tool.toml")))
            .unwrap();
    }

    fn execute(
        &mut self,
        id: &str,
        alias: &str,
        arguments: Value,
    ) -> oneshot::Receiver<Result<ToolExecuteResponse, ToolError>> {
        let (sender, receiver) = oneshot::channel();
        self.app.world().emit_event(ToolExecuteRequest {
            id: id.into(),
            agent: self.agent,
            turn: "test-turn".into(),
            alias: alias.into(),
            call_id: format!("call-{id}"),
            arguments,
            reply: Arc::new(Mutex::new(Some(sender))),
        });
        self.app.tick();
        receiver
    }

    fn response(
        &mut self,
        mut reply: oneshot::Receiver<Result<ToolExecuteResponse, ToolError>>,
    ) -> Result<ToolExecuteResponse, ToolError> {
        let deadline = Instant::now() + Duration::from_secs(10);
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

    fn tasks(&self) -> ToolTasks {
        self.app
            .world()
            .get_component::<ToolTasks>(self.agent)
            .unwrap()
            .clone()
    }

    fn removed(&mut self, id: &str) {
        let tasks = self.tasks();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            self.app.tick();
            match tasks.read(id) {
                Err(error) => {
                    assert_eq!(error.kind, Kind::TaskMissing, "{id}: {error:?}");
                    break;
                }
                Ok(result) => assert!(
                    Instant::now() < deadline,
                    "task {id} still retained: {:?}",
                    result.error
                ),
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(!tasks.list().iter().any(|(task_id, _)| task_id == id));
        assert_eq!(
            tasks.send(id, Value::Null).unwrap_err().kind,
            Kind::TaskMissing
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        assert_eq!(
            runtime.block_on(tasks.finish(id)).unwrap_err().kind,
            Kind::TaskMissing
        );
    }

    fn bundle(&mut self, alias: &str) {
        if self
            .app
            .world()
            .get_component::<Tools>(self.agent)
            .is_some_and(|tools| tools.get(alias).is_some())
        {
            return;
        }
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../../resources/tools/local")
            .join(alias)
            .join("latest/tool.toml")
            .canonicalize()
            .unwrap();
        let directory = self.root.path().join("resources").join(alias);
        shell_client::copy_resource(path.parent().unwrap(), &directory);
        let path = directory.join("tool.toml");
        if !self.app.world().contains_component::<Tools>(self.agent) {
            self.app
                .world_mut()
                .insert_component(self.agent, Tools::new(crate::test_probe()));
        }
        self.app
            .world_mut()
            .get_component_mut::<Tools>(self.agent)
            .unwrap()
            .insert(alias.into(), imported_entry(&path))
            .unwrap();
    }

    fn call(&mut self, alias: &str, arguments: Value) -> ToolExecuteResponse {
        self.bundle(alias);
        let id = format!("{alias}-{}", self.tasks().list().len());
        let reply = self.execute(&id, alias, arguments);
        let result = self.response(reply).unwrap();
        self.removed(&id);

        result
    }

    fn fails(&mut self, alias: &str, arguments: Value) -> bool {
        self.bundle(alias);
        let id = format!("{alias}-failure-{}", self.tasks().list().len());
        let reply = self.execute(&id, alias, arguments);
        let result = self.response(reply);
        self.removed(&id);
        match result {
            Ok(response) => {
                assert_eq!(response.call_id, format!("call-{id}"));
                assert_eq!(response.alias, alias);
                assert!(response.images.is_empty());
                response.failed
            }
            Err(error) => {
                assert!(!error.message.contains("stack traceback"), "{error}");
                assert!(!error.message.contains("main.lua:"), "{error}");
                true
            }
        }
    }

    fn sandbox(&mut self, name: &str) {
        use sandbox_resource_core::Sandboxes;
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../../resources/sandboxes/local")
            .join(name)
            .join("latest/policy.json")
            .canonicalize()
            .unwrap();
        let mut entries = Sandboxes::new(crate::test_probe());
        entries.insert(name.into(), path);
        self.app.world_mut().insert_component(self.agent, entries);
        self.app.world_mut().insert_component(
            self.agent,
            sandbox_resource_core::SelectedSandbox::new(Some(name.into()), crate::test_probe()),
        );
    }
}

#[test]
fn file_tools_write_read_edit_and_report_failures_without_losing_data() {
    let mut f = Fixture::new();
    let arguments = json!({"path":"nested folder/notes.txt","content":"first\n你好 world\nlast\n","create_parent_directories":true});
    let result = f.call("write-file", arguments);
    assert!(!result.failed, "{}", result.content);
    let read = f.call(
        "read-file",
        json!({"path":"nested folder/notes.txt","offset":2,"limit":1}),
    );
    assert!(!read.failed, "{}", read.content);
    assert!(read.content.contains("你好 world"));
    assert!(!read.content.contains("first"));
    let changed = f.call(
        "edit",
        json!({"path":"nested folder/notes.txt","old_string":"world","new_string":"task"}),
    );
    assert!(!changed.failed, "{}", changed.content);
    let before = std::fs::read(f.root.path().join("nested folder/notes.txt")).unwrap();
    let missing = f.call(
        "edit",
        json!({"path":"nested folder/notes.txt","old_string":"absent","new_string":"x"}),
    );
    assert!(missing.failed);
    assert_eq!(
        std::fs::read(f.root.path().join("nested folder/notes.txt")).unwrap(),
        before
    );
    assert!(f.fails("read-file", json!({"path":"missing.txt"})));
}

#[test]
fn edits_require_unique_matches_and_replace_all_and_empty_files_work() {
    let mut f = Fixture::new();
    std::fs::write(f.root.path().join("repeat.txt"), "aa bb aa").unwrap();
    let ambiguous = f.call(
        "edit",
        json!({"path":"repeat.txt","old_string":"aa","new_string":"cc"}),
    );
    assert!(ambiguous.failed);
    assert_eq!(
        std::fs::read_to_string(f.root.path().join("repeat.txt")).unwrap(),
        "aa bb aa"
    );
    assert!(
        !f.call(
            "edit",
            json!({"path":"repeat.txt","old_string":"aa","new_string":"cc","replace_all":true})
        )
        .failed
    );
    assert_eq!(
        std::fs::read_to_string(f.root.path().join("repeat.txt")).unwrap(),
        "cc bb cc"
    );
    assert!(
        !f.call("write-file", json!({"path":"empty.txt","content":""}))
            .failed
    );
    let empty = f.call("read-file", json!({"path":"empty.txt"}));
    assert!(!empty.failed, "{}", empty.content);
    let json_text = "{\"ok\":false,\"content\":\"literal file text\"}";
    assert!(
        !f.call("write-file", json!({"path":"json.txt","content":json_text}))
            .failed
    );
    let read = f.call("read-file", json!({"path":"json.txt"}));
    assert!(!read.failed);
    assert!(read.content.contains(json_text));
}

#[test]
fn files_are_bounded_and_failed_writes_do_not_report_success() {
    let mut f = Fixture::new();
    let file = std::fs::File::create(f.root.path().join("large.txt")).unwrap();
    file.set_len(16 * 1024 * 1024 + 1).unwrap();
    assert!(f.fails("read-file", json!({"path":"large.txt"})));
    assert!(f.fails(
        "edit",
        json!({"path":"large.txt","old_string":"a","new_string":"b"})
    ));
    assert!(f.fails(
        "write-file",
        json!({"path":"absent/child.txt","content":"value"})
    ));
    assert!(!f.root.path().join("absent").exists());
    #[cfg(target_os = "linux")]
    {
        assert!(f.fails(
            "write-file",
            json!({"path":"/dev/full","content":"a failed buffered write"}),
        ));
    }
}

#[test]
fn search_and_directory_tools_handle_spaces_hidden_files_and_brace_globs() {
    let mut f = Fixture::new();
    std::fs::create_dir(f.root.path().join("src folder")).unwrap();
    std::fs::write(f.root.path().join("src folder/a.ts"), "needle α\n").unwrap();
    std::fs::write(f.root.path().join("src folder/b.tsx"), "needle β\n").unwrap();
    std::fs::write(f.root.path().join("src folder/c.rs"), "needle excluded\n").unwrap();
    std::fs::write(f.root.path().join(".hidden.ts"), "hidden\n").unwrap();
    let found = f.call("glob", json!({"pattern":"*.ts","path":"."}));
    assert!(!found.failed, "{}", found.content);
    assert!(found.content.contains(".hidden.ts"));
    assert!(found.content.contains("a.ts"));
    let found = f.call(
        "grep",
        json!({"pattern":"needle","path":"src folder","include":"*.{ts,tsx}"}),
    );
    assert!(!found.failed, "{}", found.content);
    assert!(found.content.contains("a.ts"));
    assert!(found.content.contains("b.tsx"));
    assert!(!found.content.contains("c.rs"));
    assert!(
        !f.call("grep", json!({"pattern":"not-present","path":"."}))
            .failed
    );
    assert!(
        !f.call("glob", json!({"pattern":"*.absent","path":"."}))
            .failed
    );
    assert!(f.fails("grep", json!({"pattern":"[","path":"."})));
    let listed = f.call("list-directory", json!({"path":"src folder"}));
    assert!(!listed.failed, "{}", listed.content);
    assert!(listed.content.contains("a.ts"));
    assert!(f.fails("list-directory", json!({"path":"absent"})));
    std::fs::create_dir(f.root.path().join("empty")).unwrap();
    assert_eq!(
        f.call("list-directory", json!({"path":"empty"})).content,
        "[]"
    );
}

#[test]
fn all_supported_image_formats_return_actual_binary_images_and_reject_bad_inputs() {
    let mut f = Fixture::new();
    let samples: [(&str, &str, &[u8]); 4] = [
        ("tiny.png", "image/png", b"\x89PNG\r\n\x1a\nfixture"),
        ("tiny.jpg", "image/jpeg", b"\xff\xd8\xfffixture"),
        ("tiny.gif", "image/gif", b"GIF89afixture"),
        ("tiny.webp", "image/webp", b"RIFF\0\0\0\0WEBPfixture"),
    ];
    for (name, media, bytes) in samples {
        std::fs::write(f.root.path().join(name), bytes).unwrap();
        let result = f.call("read-image", json!({"path":name}));
        assert!(!result.failed, "{}", result.content);
        assert_eq!(result.images.len(), 1);
        assert_eq!(result.images[0].media_type, media);
        use base64::Engine;
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(&result.images[0].data)
                .unwrap(),
            bytes
        );
    }
    std::fs::write(f.root.path().join("text.png"), "not an image").unwrap();
    assert!(f.fails("read-image", json!({"path":"text.png"})));
    std::fs::write(f.root.path().join("empty.png"), []).unwrap();
    assert!(f.fails("read-image", json!({"path":"empty.png"})));
    let file = std::fs::File::create(f.root.path().join("huge.png")).unwrap();
    file.set_len(5 * 1024 * 1024 + 1).unwrap();
    assert!(f.fails("read-image", json!({"path":"huge.png"})));
}

#[test]
fn standard_guidance_skills_and_actual_resource_schemas_are_available() {
    let mut f = Fixture::new();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../resources");
    for name in ["code-review", "project-context"] {
        let content =
            std::fs::read_to_string(root.join("skills/local").join(name).join("SKILL.md")).unwrap();
        assert!(content.starts_with(&format!("---\nname: {name}\n")));
        assert!(content.contains("Guidance"));
        assert!(!root.join("tools/local").join(name).exists());
    }
    for alias in [
        "read-file",
        "read-image",
        "edit",
        "write-file",
        "task-read",
        "task-send",
        "task-finish",
    ] {
        f.bundle(alias);
        let id = format!("invalid-{alias}");
        let reply = f.execute(&id, alias, json!({}));
        assert_eq!(f.response(reply).unwrap_err().kind, Kind::InvalidArguments);
        f.removed(&id);
    }
}

#[test]
fn task_control_tools_pass_structured_messages_null_and_shared_cache_cursors() {
    let mut f = Fixture::new();
    f.tool(
        "worker",
        r#"function execute(arguments, context)
        margatroid.task:return_result(margatroid.context.task_id)
        while true do
            local value = margatroid.task:receive()
            if type(value) == 'table' and value.finish then break end
            if type(value) == 'table' and value.replace then margatroid.task:replace_output(value.value)
            else margatroid.task:append_output(value) end
        end
        margatroid.task:finish(margatroid.context.task_id)
    end"#,
    );
    let reply = f.execute("worker", "worker", json!({}));
    f.response(reply).unwrap();
    assert!(
        !f.call(
            "task-send",
            json!({"task_id":"worker","value":{"answer":42}})
        )
        .failed
    );
    assert!(
        !f.call("task-send", json!({"task_id":"worker","value":null}))
            .failed
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut received = Vec::new();
    while received.len() < 2 {
        let response = f.call("task-read", json!({"task_id":"worker"}));
        let value: Value = serde_json::from_str(&response.content).unwrap();
        assert!(value.get("completed").is_none());
        received.extend(value["output"].as_array().unwrap().clone());
        assert!(Instant::now() < deadline);
    }
    assert_eq!(received, vec![json!({"answer":42}), Value::Null]);
    let response = f.call("task-read", json!({"task_id":"worker"}));
    assert_eq!(
        serde_json::from_str::<Value>(&response.content).unwrap()["output"],
        json!([])
    );
    f.call(
        "task-send",
        json!({"task_id":"worker","value":{"replace":true,"value":"new screen"}}),
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let result = f.call("task-read", json!({"task_id":"worker"}));
        let result: Value = serde_json::from_str(&result.content).unwrap();
        if result["replaced"] == true {
            assert_eq!(result["output"], json!(["new screen"]));
            break;
        }
        assert!(Instant::now() < deadline);
    }
    assert!(!f.call("task-finish", json!({"task_id":"worker"})).failed);
    f.removed("worker");
    for (index, (alias, arguments)) in [
        ("task-read", json!({"task_id":"worker"})),
        ("task-finish", json!({"task_id":"worker"})),
        ("task-send", json!({"task_id":"worker","value":"late"})),
        ("task-read", json!({"task_id":"unknown"})),
    ]
    .into_iter()
    .enumerate()
    {
        f.bundle(alias);
        let id = format!("missing-{index}");
        let reply = f.execute(&id, alias, arguments);
        let failure = f.response(reply).unwrap_err();
        assert_eq!(failure.kind, tool_core::ToolErrorKind::TaskMissing);
        assert_eq!(failure.to_string(), "TaskMissing: tool task not found");
        assert!(!failure.message.contains("stack traceback"));
        f.removed(&id);
    }
}

#[test]
fn actual_bash_resource_interacts_through_actual_task_control_tools() {
    let mut f = Fixture::new();
    f.bundle("bash");
    let reply = f.execute("bash-live", "bash", json!({}));
    let reply = f.response(reply).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&reply.content).unwrap()["task_id"],
        "bash-live"
    );
    assert!(!f.call("task-send",json!({"task_id":"bash-live","value":{"text":"export BUNDLED_TEST=kept; mkdir -p 'folder with spaces'; cd 'folder with spaces'","enter":true}})).failed);
    assert!(!f.call("task-send",json!({"task_id":"bash-live","value":{"text":"printf '\\nRESULT=%s CWD=%s\\n' \"$BUNDLED_TEST\" \"${PWD##*/}\"","enter":true}})).failed);
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut output = String::new();
    while !output.contains("RESULT=kept CWD=folder with spaces") {
        let response = f.call("task-read", json!({"task_id":"bash-live"}));
        let response: Value = serde_json::from_str(&response.content).unwrap();
        if response["replaced"] == true {
            output.clear();
        }
        for value in response["output"].as_array().unwrap() {
            if let Some(text) = value.as_str() {
                output.push_str(text);
            }
        }
        assert!(Instant::now() < deadline, "{output}");
    }
    assert!(!f.call("task-finish", json!({"task_id":"bash-live"})).failed);
    f.removed("bash-live");
}

#[test]
fn real_workspace_write_and_read_only_sandboxes_enforce_file_access() {
    let mut f = Fixture::new();
    std::fs::write(f.root.path().join("original.txt"), "original").unwrap();
    f.sandbox("workspace-write");
    assert!(
        !f.call(
            "write-file",
            json!({"path":"allowed.txt","content":"sandbox write"})
        )
        .failed
    );
    assert_eq!(
        std::fs::read_to_string(f.root.path().join("allowed.txt")).unwrap(),
        "sandbox write"
    );
    assert!(!f.call("read-file", json!({"path":"original.txt"})).failed);
    let outside = tempfile::tempdir().unwrap();
    let outside_file = outside.path().join("outside.txt");
    std::fs::write(&outside_file, "untouched").unwrap();
    assert!(f.fails(
        "write-file",
        json!({"path": outside_file, "content":"forbidden"})
    ));
    assert_eq!(std::fs::read_to_string(&outside_file).unwrap(), "untouched");
    f.sandbox("read-only");
    assert!(!f.call("read-file", json!({"path":"original.txt"})).failed);
    assert!(f.fails(
        "write-file",
        json!({"path":"original.txt","content":"forbidden"})
    ));
    assert_eq!(
        std::fs::read_to_string(f.root.path().join("original.txt")).unwrap(),
        "original"
    );
}

#[test]
fn text_boundaries_and_literal_replacements_preserve_bytes_on_failure() {
    let mut f = Fixture::new();
    std::fs::write(f.root.path().join("crlf.txt"), "first\r\n第二行\r\n").unwrap();
    let result = f.call("read-file", json!({"path":"crlf.txt","offset":2,"limit":1}));
    assert!(!result.failed);
    assert!(!result.content.contains('\r'));
    assert!(result.content.contains("第二行"));
    assert!(f
        .call("read-file", json!({"path":"crlf.txt","offset":99}))
        .content
        .contains("(end of file)"));
    std::fs::write(f.root.path().join("bad.txt"), b"\xff\xfe").unwrap();
    assert!(f.fails("read-file", json!({"path":"bad.txt"})));
    assert!(f.fails(
        "edit",
        json!({"path":"bad.txt","old_string":"a","new_string":"b"})
    ));
    std::fs::write(f.root.path().join("long.txt"), "x".repeat(256 * 1024 + 1)).unwrap();
    assert!(f.fails("read-file", json!({"path":"long.txt"})));
    std::fs::write(f.root.path().join("literal.txt"), "a.b a.b").unwrap();
    assert!(!f.call("edit",json!({"path":"literal.txt","old_string":"a.b","new_string":"%1 literal","replace_all":true})).failed);
    assert_eq!(
        std::fs::read_to_string(f.root.path().join("literal.txt")).unwrap(),
        "%1 literal %1 literal"
    );
    let before = std::fs::read(f.root.path().join("literal.txt")).unwrap();
    f.bundle("edit");
    let reply = f.execute(
        "empty-needle",
        "edit",
        json!({"path":"literal.txt","old_string":"","new_string":"bad"}),
    );
    assert_eq!(f.response(reply).unwrap_err().kind, Kind::InvalidArguments);
    f.removed("empty-needle");
    assert_eq!(
        std::fs::read(f.root.path().join("literal.txt")).unwrap(),
        before
    );
    let large = std::fs::File::create(f.root.path().join("expansion.txt")).unwrap();
    large.set_len(16 * 1024 * 1024).unwrap();
    let result=f.call("edit",json!({"path":"expansion.txt","old_string":"\u{0000}","new_string":"ab","replace_all":true}));
    assert!(result.failed, "{}", result.content);
    assert_eq!(
        std::fs::metadata(f.root.path().join("expansion.txt"))
            .unwrap()
            .len(),
        16 * 1024 * 1024
    );
}

#[test]
fn directory_and_search_boundaries_handle_unusual_names_and_limit_results() {
    let mut f = Fixture::new();
    let unusual = "a 'quote\tand\nnewline.ts";
    std::fs::write(f.root.path().join(unusual), "needle\n").unwrap();
    let list = f.call("list-directory", json!({"path":"."}));
    let entries: Value = serde_json::from_str(&list.content).unwrap();
    assert!(entries
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["name"] == unusual));
    let found = f.call("glob", json!({"pattern":"*.ts"}));
    assert!(!found.failed, "{}", found.content);
    assert!(found.content.contains("newline.ts"));
    assert!(found.content.contains("\\n"));
    std::fs::write(f.root.path().join("many.txt"), "match 中文\n".repeat(251)).unwrap();
    let found = f.call("grep", json!({"pattern":"match","path":"many.txt"}));
    assert!(!found.failed, "{}", found.content);
    assert!(found.content.contains("additional") || found.content.contains("omitted"));
    assert_eq!(
        found
            .content
            .lines()
            .filter(|line| line.contains("many.txt:"))
            .count(),
        250
    );
    std::fs::write(
        f.root.path().join("utf8.txt"),
        format!("needle{}", "中".repeat(1000)),
    )
    .unwrap();
    let found = f.call("grep", json!({"pattern":"needle","path":"utf8.txt"}));
    assert!(!found.failed, "{}", found.content);
    assert!(found.content.contains("truncated"));
    let many = f.root.path().join("many");
    std::fs::create_dir(&many).unwrap();
    for index in 0..101 {
        std::fs::write(many.join(format!("{index:03}.txt")), "value").unwrap();
    }
    let found = f.call("glob", json!({"pattern":"*.txt","path":"many"}));
    assert!(!found.failed, "{}", found.content);
    assert_eq!(
        found
            .content
            .lines()
            .filter(|line| line.ends_with(".txt"))
            .count(),
        100
    );
    assert!(found.content.contains("additional") || found.content.contains("omitted"));
}

#[test]
fn a_real_png_is_returned_unchanged_and_does_not_depend_on_extension() {
    use base64::Engine;
    let mut f = Fixture::new();
    let encoded = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a6l8AAAAASUVORK5CYII=";
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .unwrap();
    std::fs::write(f.root.path().join("image.data"), &bytes).unwrap();
    let result = f.call("read-image", json!({"path":"image.data"}));
    assert!(!result.failed, "{}", result.content);
    assert_eq!(result.images[0].media_type, "image/png");
    assert_eq!(result.images[0].data, encoded);
}

#[test]
fn workspace_sandbox_bash_handles_input_errors_and_tui_updates_then_closes() {
    let mut f = Fixture::new();
    f.sandbox("workspace-write");
    f.bundle("bash");
    let reply = f.execute("sandbox-bash", "bash", json!({}));
    f.response(reply).unwrap();
    assert!(
        !f.call(
            "task-send",
            json!({"task_id":"sandbox-bash","value":{"text":"echo should-not-run","enter":"yes"}})
        )
        .failed
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let response = f.call("task-read", json!({"task_id":"sandbox-bash"}));
        let value: Value = serde_json::from_str(&response.content).unwrap();
        if value["output"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.get("error").is_some())
        {
            break;
        }
        assert!(Instant::now() < deadline);
    }
    f.call("task-send",json!({"task_id":"sandbox-bash","value":{"text":"printf '\\033[?1049h\\033[2J\\033[HREAL_TUI_SCREEN'; sleep 0.1","enter":true}}));
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let response = f.call("task-read", json!({"task_id":"sandbox-bash"}));
        let value: Value = serde_json::from_str(&response.content).unwrap();
        let screen = value["output"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect::<String>();
        assert!(!screen.contains('\u{1b}'));
        if value["replaced"] == true && screen.contains("REAL_TUI_SCREEN") {
            break;
        }
        assert!(Instant::now() < deadline, "{value}");
    }
    assert!(
        !f.call("task-finish", json!({"task_id":"sandbox-bash"}))
            .failed
    );
    f.removed("sandbox-bash");
}

#[test]
fn one_shot_bash_supports_bash_syntax_stderr_failures_and_does_not_persist_state() {
    let mut f = Fixture::new();
    f.sandbox("workspace-write");
    let result = f.call(
        "bash-exec",
        json!({"cmd":"values=(one two); printf '%s' \"${values[1]}\"; printf 'diagnostic' >&2"}),
    );
    assert!(!result.failed, "{}", result.content);
    assert!(result.content.contains("two"));
    assert!(result.content.contains("[stderr]\ndiagnostic"));
    assert!(result.content.contains("[exit code: 0]"));
    let result = f.call(
        "bash-exec",
        json!({"cmd":"printf 'failed detail' >&2; exit 7"}),
    );
    assert!(result.failed);
    assert!(result.content.contains("failed detail"));
    assert!(result.content.contains("[exit code: 7]"));
    assert!(
        !f.call(
            "bash-exec",
            json!({"cmd":"export ONESHOT_TEST=gone; mkdir -p once; cd once"})
        )
        .failed
    );
    let result = f.call(
        "bash-exec",
        json!({"cmd":"printf 'VALUE=%s DIR=%s' \"${ONESHOT_TEST-unset}\" \"$PWD\""}),
    );
    assert!(result.content.contains("VALUE=unset"));
    assert!(result
        .content
        .contains(&format!("DIR={}", f.root.path().display())));
    let result = f.call("bash-exec", json!({"cmd":"true"}));
    assert!(result.content.contains("command completed with no output"));
    assert!(f.tasks().list().is_empty());
}

#[test]
fn one_shot_graphics_output_is_bounded_without_exceeding_result_encoding_budget() {
    let mut f = Fixture::new();
    let result = f.call("bash-exec", json!({"cmd":"python3 -c \"import sys;sys.stdout.write('\\x1b' * 200000);sys.stderr.write('\\x00' * 200000)\""}));
    assert!(!result.failed);
    assert!(result
        .content
        .contains("[output truncated at the size limit]"));
    assert!(result.content.contains("[exit code: 0]"));
    assert!(result.content.len() < 140000);
}

#[test]
fn explicit_tool_content_is_preserved_and_unexpected_lua_errors_are_brief() {
    let mut f = Fixture::new();
    let text = "TaskMissing: example\nstack traceback:\n\t[C]: in function 'task_read'";
    for (alias, ok) in [("reported-failure", false), ("reported-success", true)] {
        let literal = serde_json::to_string(text).unwrap();
        f.tool(alias, &format!("function execute() margatroid.task:return_result({{ok={ok},content={literal}}}); margatroid.task:finish(margatroid.context.task_id) end"));
        let reply = f.execute(alias, alias, json!({}));
        let response = f.response(reply).unwrap();
        assert_eq!(response.call_id, format!("call-{alias}"));
        assert_eq!(response.alias, alias);
        assert_eq!(response.failed, !ok);
        assert_eq!(response.content, text);
        f.removed(alias);
    }
    f.tool(
        "lua-error",
        "function execute() error('operator failure') end",
    );
    let reply = f.execute("lua-error", "lua-error", json!({}));
    let failure = f.response(reply).unwrap_err();
    assert_eq!(failure.kind, tool_core::ToolErrorKind::ExecutionFailed);
    assert!(failure.message.contains("operator failure"));
    assert!(!failure.message.contains("stack traceback:"));
    f.removed("lua-error");
}

#[test]
fn bundled_file_tools_execute_without_shell_helpers_or_packaged_binaries() {
    let mut f = Fixture::new();
    for alias in [
        "read-file",
        "write-file",
        "edit",
        "read-image",
        "list-directory",
        "glob",
        "grep",
    ] {
        f.bundle(alias);
        let resource = f.root.path().join("resources").join(alias);
        assert!(
            !resource.join("shell.lua").exists(),
            "{alias} retains a shell helper"
        );
        assert!(
            !resource.join("bin").exists(),
            "{alias} fixture installed a binary"
        );
        // Remove an obsolete installation before invoking the actual bundled script.
        let bin = resource.join("bin");
        std::fs::create_dir(&bin).unwrap();
        std::fs::write(bin.join("obsolete"), "unused").unwrap();
        assert_eq!(bin.parent(), Some(resource.as_path()));
        std::fs::remove_dir_all(&bin).unwrap();
    }
    assert!(
        !f.call(
            "write-file",
            json!({"path":"plain.txt","content":"needle α\n"})
        )
        .failed
    );
    assert!(!f.call("read-file", json!({"path":"plain.txt"})).failed);
    assert!(
        !f.call(
            "edit",
            json!({"path":"plain.txt","old_string":"α","new_string":"β"})
        )
        .failed
    );
    assert!(!f.call("list-directory", json!({"path":"."})).failed);
    assert!(f
        .call("glob", json!({"pattern":"*.txt"}))
        .content
        .contains("plain.txt"));
    assert!(f
        .call("grep", json!({"pattern":"needle β","path":"plain.txt"}))
        .content
        .contains("plain.txt:1:needle β"));
    std::fs::write(f.root.path().join("tiny.png"), b"\x89PNG\r\n\x1a\nfixture").unwrap();
    assert_eq!(
        f.call("read-image", json!({"path":"tiny.png"}))
            .images
            .len(),
        1
    );
}

#[test]
fn bundled_grep_preserves_nested_ignore_negations_binary_lines_unicode_and_include() {
    let mut f = Fixture::new();
    std::fs::create_dir_all(f.root.path().join("nested/deeper")).unwrap();
    std::fs::write(
        f.root.path().join(".ignore"),
        "*.skip\n*.log\n/root-only.txt\n",
    )
    .unwrap();
    std::fs::write(
        f.root.path().join("nested/.ignore"),
        "!keep.log\nprivate.txt\n",
    )
    .unwrap();
    for (path, text) in [
        ("ignored.skip", "needle ignored"),
        ("nested/discard.log", "needle ignored"),
        ("nested/keep.log", "needle 保留"),
        ("nested/private.txt", "needle ignored"),
        ("nested/deeper/keep.log", "needle inherited negation"),
        ("visible.rs", "header\r\nneedle α needle β\r\n尾行\n"),
        ("other.txt", "needle other"),
        ("root-only.txt", "needle root ignored"),
        ("nested/root-only.txt", "needle nested included"),
        (".hidden.txt", "needle hidden"),
    ] {
        std::fs::write(f.root.path().join(path), text).unwrap();
    }
    std::fs::write(f.root.path().join("binary.txt"), b"\0needle binary\n").unwrap();
    std::fs::write(f.root.path().join("nonutf8.txt"), b"needle \xff\n").unwrap();
    let found = f.call("grep", json!({"pattern":"needle","path":"."}));
    assert!(!found.failed, "{}", found.content);
    assert!(
        found.content.contains("nested/keep.log:1:needle 保留"),
        "{}",
        found.content
    );
    assert!(
        found
            .content
            .contains("nested/deeper/keep.log:1:needle inherited negation"),
        "{}",
        found.content
    );
    assert!(
        found.content.contains("visible.rs:2:needle α needle β"),
        "{}",
        found.content
    );
    assert!(
        found.content.contains("nested/root-only.txt:1:"),
        "{}",
        found.content
    );
    assert!(
        !found
            .content
            .lines()
            .any(|line| line.starts_with("root-only.txt:") || line.starts_with("./root-only.txt:")),
        "{}",
        found.content
    );
    assert_eq!(
        found
            .content
            .lines()
            .filter(|line| line.contains("visible.rs:"))
            .count(),
        1
    );
    // rg --json emits matching binary lines; non-UTF-8 previews retain base64 bytes.
    assert!(
        found.content.contains("binary.txt:1:\0needle binary"),
        "{}",
        found.content
    );
    assert!(
        found
            .content
            .contains("nonutf8.txt:1:[base64 bytes: bmVlZGxlIP8K]"),
        "{}",
        found.content
    );
    for excluded in ["ignored.skip", "discard.log", "private.txt", ".hidden.txt"] {
        assert!(
            !found.content.contains(excluded),
            "{excluded}: {}",
            found.content
        );
    }
    let included = f.call(
        "grep",
        json!({"pattern":"(?i)NEEDLE\\s[αβ]","include":"*.{rs,ts}"}),
    );
    assert!(!included.failed, "{}", included.content);
    assert!(
        included.content.contains("visible.rs:2:"),
        "{}",
        included.content
    );
    assert!(!included.content.contains("other.txt"));
    assert!(f.fails("grep", json!({"pattern":"needle","include":"!*.rs"})));
}

#[test]
fn bundled_glob_includes_ignored_files_excludes_vcs_directories_and_sorts_nanoseconds() {
    let mut f = Fixture::new();
    std::fs::create_dir_all(f.root.path().join("nested/.git")).unwrap();
    std::fs::create_dir_all(f.root.path().join("nested/sub")).unwrap();
    std::fs::write(f.root.path().join(".ignore"), "*.txt\n").unwrap();
    for name in [
        "z-new.txt",
        "a-old.txt",
        "nested/.hidden.txt",
        "nested/.git/internal.txt",
    ] {
        std::fs::write(f.root.path().join(name), "file").unwrap();
    }
    // Both mtimes have the same second and differ below floating-point precision.
    let second = std::time::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    for (name, nanos) in [("z-new.txt", 200), ("a-old.txt", 100)] {
        std::fs::File::open(f.root.path().join(name))
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(second + Duration::from_nanos(nanos)))
            .unwrap();
    }
    let found = f.call("glob", json!({"pattern":"*.txt"}));
    assert!(!found.failed, "{}", found.content);
    assert!(
        found.content.contains("nested/.hidden.txt"),
        "{}",
        found.content
    );
    assert!(!found.content.contains("internal.txt"));
    let paths: Vec<_> = found.content.lines().collect();
    let new = paths
        .iter()
        .position(|path| path.ends_with("z-new.txt"))
        .unwrap();
    let old = paths
        .iter()
        .position(|path| path.ends_with("a-old.txt"))
        .unwrap();
    assert!(new < old, "{}", found.content);
    let anchored = f.call("glob", json!({"pattern":"nested/*.txt"}));
    assert!(anchored.content.contains("nested/.hidden.txt"));
    assert!(!anchored.content.contains("z-new.txt"));
    assert!(f.fails("glob", json!({"pattern":"["})));
}

#[test]
fn explicit_file_roots_override_grep_include_and_glob_patterns() {
    let mut f = Fixture::new();
    std::fs::write(f.root.path().join(".ignore"), "note.md\n").unwrap();
    std::fs::write(f.root.path().join("note.md"), "needle explicit\n").unwrap();
    let direct = f.call(
        "grep",
        json!({"pattern":"needle","path":"note.md","include":"*.txt"}),
    );
    assert!(!direct.failed, "{}", direct.content);
    assert!(
        direct.content.contains("note.md:1:needle explicit"),
        "{}",
        direct.content
    );
    let directory = f.call(
        "grep",
        json!({"pattern":"needle","path":".","include":"*.txt"}),
    );
    assert!(!directory.failed, "{}", directory.content);
    assert_eq!(directory.content, "No matches found.");
    let direct = f.call("glob", json!({"pattern":"*.txt","path":"note.md"}));
    assert!(!direct.failed, "{}", direct.content);
    assert!(direct.content.contains("note.md"), "{}", direct.content);
    let directory = f.call("glob", json!({"pattern":"*.txt","path":"."}));
    assert!(!directory.failed, "{}", directory.content);
    assert_eq!(directory.content, "No files found.");
}

#[test]
fn bundled_search_host_failures_keep_tracebacks_in_storage_and_brief_call_errors() {
    for (alias, arguments) in [
        ("grep", json!({"pattern":"[","path":"."})),
        (
            "grep",
            json!({"pattern":"needle","path":"missing-search-root"}),
        ),
        ("glob", json!({"pattern":"[","path":"."})),
    ] {
        let mut f = Fixture::new();
        f.bundle(alias);
        let reply = f.execute("search-failure", alias, arguments);
        let error = f.response(reply).unwrap_err();
        f.removed("search-failure");
        assert!(!error.message.contains("stack traceback"), "{error}");
        assert!(!error.message.contains("main.lua:"), "{error}");
        assert!(!error.message.contains("[string"), "{error}");
        let storage = f
            .app
            .world()
            .get_component::<storage_core::AgentStorage>(f.agent)
            .unwrap();
        let logs = storage_core::read_agent_logs(storage, None, 100).unwrap();
        assert_eq!(logs.len(), 1, "{logs:?}");
        let record = &logs[0].record;
        assert_eq!(record.source, storage_core::AgentLogSource::Tool);
        assert_eq!(record.stage, "execute");
        assert!(record.origin.as_deref().unwrap().ends_with("main.lua"));
        assert!(
            record
                .raw_error
                .as_deref()
                .unwrap()
                .contains("stack traceback:"),
            "{record:?}"
        );
        assert!(
            record
                .traceback
                .as_deref()
                .unwrap()
                .contains("stack traceback:"),
            "{record:?}"
        );
        let context = record.context_json.as_ref().unwrap();
        assert_eq!(context["task_id"], "search-failure");
        assert_eq!(context["call_id"], "call-search-failure");
        assert_eq!(context["alias"], alias);
    }
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

fn test_agent_context() -> agent_core::AgentUpdateContext {
    agent_core::AgentUpdateContext::new(|_, _, _| Vec::new(), |_, _| {})
}

fn test_workspace_context() -> workspace_core::WorkspaceUpdateContext {
    workspace_core::WorkspaceUpdateContext::new(|_, _| {})
}

fn test_probe<T>() -> client_core::UpdateProbe<T> {
    client_core::UpdateProbe::new(|_| {})
}
