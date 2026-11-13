//! Pure Lua skill resource integration through the actual tool host.
#[path = "support/storage.rs"]
mod storage_fixture;

#[cfg(target_os = "linux")]
mod linux {
    use agent_core::AgentState;
    use app_runtime_plugin::RuntimePlugin;
    use async_runtime_plugin::AsyncRuntimePlugin;
    use closure_plugin::{AppClosureExt, ClosurePlugin};
    use core_plugin::{App, Entity};
    use serde_json::{json, Value};
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
    use tool_resource_core::{read_tool, ToolEntry, Tools};
    use workspace_core::WorkspaceState;

    type Reply = oneshot::Receiver<Result<ToolExecuteResponse, ToolError>>;
    const BUDGET: Duration = Duration::from_secs(30);
    const ALIAS: &str = "host-test-skill";
    const DESCRIPTION: &str = "Read host integration instructions and references.";
    const REFERENCE: &str =
        "附属资料：真实 UTF-8 内容。\nRead this reference after the instructions.\n";

    struct Fixture {
        app: App,
        agent: Entity,
        root: tempfile::TempDir,
        resource: PathBuf,
        deadline: Instant,
    }
    impl Fixture {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let resource = root.path().join("skill-register/latest");
            std::fs::create_dir_all(&resource).unwrap();
            let bundled = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../../../resources/tools/local/skill-register/latest");
            for entry in std::fs::read_dir(&bundled).unwrap() {
                let entry = entry.unwrap();
                let name = entry.file_name();
                if entry.file_type().unwrap().is_file()
                    && (name == "tool.toml"
                        || name == "input.schema.json"
                        || entry
                            .path()
                            .extension()
                            .is_some_and(|extension| extension == "lua"))
                {
                    std::fs::copy(entry.path(), resource.join(name)).unwrap();
                }
            }
            let imported = read_tool(&resource.join("tool.toml")).unwrap();
            let mut tools = Tools::new(crate::test_probe());
            tools
                .insert(
                    "skill_register".into(),
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
                    ToolPlugin::new(crate::test_binding())
                        .with_runner_path(env!("CARGO_BIN_EXE_tool_runner")),
                );
            let workspace = app.world_mut().spawn();
            app.world_mut().insert_component(
                workspace,
                workspace_core::Workspace::new(
                    "skill-integration".into(),
                    root.path().join("workspace.yaml"),
                    root.path().to_owned(),
                    WorkspaceState::Running,
                    BTreeSet::new(),
                    None,
                    crate::test_workspace_context(),
                ),
            );
            let agent = app.world_mut().spawn();
            super::storage_fixture::mount(app.world_mut(), agent, root.path());
            app.world_mut().insert_component(
                agent,
                agent_core::Agent::new(
                    "skill-integration".into(),
                    workspace,
                    AgentState::Working,
                    Some("integration-turn".into()),
                    crate::test_agent_context(),
                ),
            );
            app.world_mut().insert_component(agent, tools);
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
                resource,
                deadline: Instant::now() + BUDGET,
            }
        }
        fn tasks(&self) -> ToolTasks {
            self.app
                .world()
                .get_component::<ToolTasks>(self.agent)
                .unwrap()
                .clone()
        }
        fn tools(&self) -> Tools {
            self.app
                .world()
                .get_component::<Tools>(self.agent)
                .unwrap()
                .clone()
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
            reply
        }
        fn response(&mut self, mut reply: Reply) -> Result<ToolExecuteResponse, ToolError> {
            loop {
                self.app.tick();
                match reply.try_recv() {
                    Ok(response) => return response,
                    Err(oneshot::error::TryRecvError::Empty) => {}
                    Err(error) => panic!("tool reply closed: {error}"),
                }
                assert!(
                    Instant::now() < self.deadline,
                    "skill host test exceeded 30s budget"
                );
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        fn drained(&mut self) {
            while !self.tasks().list().is_empty() {
                self.app.tick();
                assert!(
                    Instant::now() < self.deadline,
                    "skill tasks did not finish within 30s"
                );
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        fn success(&mut self, id: &str, alias: &str, arguments: Value) -> String {
            let reply = self.execute(id, alias, arguments);
            let response = self.response(reply).unwrap();
            assert!(!response.failed, "{id}: {}", response.content);
            assert_eq!(response.alias, alias);
            assert_eq!(response.call_id, format!("call-{id}"));
            self.drained();
            response.content
        }
        fn logs(&self) -> Vec<storage_core::StoredAgentLog> {
            storage_core::read_agent_logs(
                self.app
                    .world()
                    .get_component::<storage_core::AgentStorage>(self.agent)
                    .unwrap(),
                None,
                100,
            )
            .unwrap()
        }
        fn failure(&mut self, id: &str, alias: &str, arguments: Value) -> String {
            let logs_before = self.logs();
            let reply = self.execute(id, alias, arguments);
            let response = self.response(reply).unwrap();
            assert!(
                response.failed,
                "{id} unexpectedly succeeded: {}",
                response.content
            );
            self.drained();
            assert!(
                response.content.starts_with("Skill: "),
                "{id}: {}",
                response.content
            );
            assert!(!response.content.contains("stack traceback"));
            assert!(!response.content.contains("main.lua:"));
            assert_eq!(
                self.logs(),
                logs_before,
                "validation must not be auto-logged"
            );
            response.content
        }
    }

    #[test]
    fn unexpected_skill_exception_is_rethrown_to_host_and_logged_once() {
        let mut f = Fixture::new();
        let skill = f.root.path().join("unexpected-skill");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: unexpected-skill\ndescription: valid\n---\nInstructions\n",
        )
        .unwrap();
        let program_path = f.resource.join("main.lua");
        let program = std::fs::read_to_string(&program_path).unwrap();
        let program = program.replace(
            "local files = inventory(root)",
            "error('unexpected-skill-marker')\n    local files = inventory(root)",
        );
        std::fs::write(program_path, program).unwrap();
        let reply = f.execute(
            "unexpected",
            "skill_register",
            json!({"path":skill.join("SKILL.md")}),
        );
        let failure = f.response(reply).unwrap_err();
        assert_eq!(failure.kind, ToolErrorKind::ExecutionFailed);
        assert!(!failure.message.contains("main.lua:"));
        assert!(!failure.message.contains("stack traceback"));
        f.drained();
        let logs = f.logs();
        assert_eq!(logs.len(), 1);
        assert!(logs[0]
            .record
            .raw_error
            .as_deref()
            .unwrap()
            .contains("unexpected-skill-marker"));
        assert_eq!(logs[0].record.source, storage_core::AgentLogSource::Tool);
    }

    #[test]
    fn skill_symlinks_use_canonical_targets_for_registration_inventory_and_reads() {
        use std::os::unix::fs::symlink;
        let mut f = Fixture::new();
        let skill = f.root.path().join("canonical-skill");
        std::fs::create_dir_all(skill.join("references")).unwrap();
        let instructions =
            "---\nname: canonical-skill\ndescription: Canonical skill paths.\n---\nInstructions\n";
        std::fs::write(skill.join("SKILL.md"), instructions).unwrap();
        std::fs::write(skill.join("references/guide.md"), REFERENCE).unwrap();
        symlink("references/guide.md", skill.join("linked.md")).unwrap();
        let query = f.root.path().join("query-skill");
        symlink(&skill, &query).unwrap();
        let registration = f.success(
            "symlink-register",
            "skill_register",
            json!({"path":query.join("SKILL.md")}),
        );
        let registered: Value = serde_json::from_str(&registration).unwrap();
        assert_eq!(
            registered["skill_md"],
            json!(skill.join("SKILL.md").canonicalize().unwrap())
        );
        let linked = registered["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|file| file["path"] == "linked.md")
            .unwrap();
        assert_eq!(linked["kind"], "file");
        assert_eq!(linked["size_bytes"], REFERENCE.len());
        assert_eq!(
            f.success(
                "linked-read",
                "canonical-skill",
                json!({"path":"linked.md"})
            ),
            REFERENCE
        );
        // A link retargeted after registration is rechecked against the original root.
        let outside = f.root.path().join("outside.md");
        std::fs::write(&outside, "outside").unwrap();
        let link = skill.join("linked.md");
        assert_eq!(link.parent(), Some(skill.as_path()));
        std::fs::remove_file(&link).unwrap();
        symlink(&outside, &link).unwrap();
        let failure = f.failure(
            "escaped-link",
            "canonical-skill",
            json!({"path":"linked.md"}),
        );
        assert!(failure.contains("outside skill root"), "{failure}");
    }

    #[test]
    fn skill_registration_accepts_quoted_and_multiline_yaml() {
        let mut f = Fixture::new();
        for (alias, yaml, expected) in [
            (
                "quoted-skill",
                "'Read: quoted # guidance'",
                "Read: quoted # guidance",
            ),
            (
                "multiline-skill",
                ">-\n  Read host integration\n  instructions and references.",
                DESCRIPTION,
            ),
        ] {
            let skill = f.root.path().join(alias);
            std::fs::create_dir_all(&skill).unwrap();
            let instructions =
                format!("---\nname: '{alias}'\ndescription: {yaml}\n---\n# Guidance\n");
            std::fs::write(skill.join("SKILL.md"), &instructions).unwrap();
            f.success(
                alias,
                "skill_register",
                json!({"path":skill.join("SKILL.md")}),
            );
            let tools = f.tools();
            let entry = tools.get(alias).unwrap();
            assert_eq!(entry.description, expected);
            assert_eq!(entry.owner_task_id, None);
            assert_eq!(
                f.success(&format!("read-{alias}"), alias, json!({})),
                instructions
            );
        }
    }

    #[test]
    fn skill_registration_static_binding_and_fresh_reads() {
        let mut f = Fixture::new();
        let skill = f.root.path().join(ALIAS);
        std::fs::create_dir_all(skill.join("references")).unwrap();
        std::fs::create_dir_all(skill.join("scripts")).unwrap();
        std::fs::create_dir_all(skill.join("assets")).unwrap();
        let instructions = format!(
            "---\nname: {ALIAS}\ndescription: {DESCRIPTION}\nlicense: MIT\ncompatibility: Local files only\nmetadata:\n  author: host-test\nallowed-tools: Read\n---\n# Host instructions\n\nRead references/guide.md. 指令必须保留。\n"
        );
        std::fs::write(skill.join("SKILL.md"), &instructions).unwrap();
        std::fs::write(skill.join("references/guide.md"), REFERENCE).unwrap();
        let marker = f.root.path().join("script-was-executed");
        let script = format!("#!/bin/sh\ntouch '{}'\n", marker.display());
        std::fs::write(skill.join("scripts/example.sh"), &script).unwrap();
        let binary = [0, 255, 1, 128];
        std::fs::write(skill.join("assets/sample.bin"), binary).unwrap();

        // Relative paths resolve against the actual workspace, not the process cwd.
        let registration = f.success(
            "register",
            "skill_register",
            json!({"path":format!("{ALIAS}/SKILL.md")}),
        );
        let registered: Value = serde_json::from_str(&registration).unwrap();
        assert_eq!(registered["alias"], ALIAS);
        assert_eq!(
            registered["skill_md"],
            json!(skill.join("SKILL.md").canonicalize().unwrap())
        );
        assert!(!registration.contains("Host instructions"));
        assert!(!registration.contains(REFERENCE));
        assert!(!registration.contains(&script));
        assert!(f.tasks().get("register").is_err());
        let definitions = f.tools();
        assert_eq!(definitions.iter().count(), 2);
        let entry = definitions.get(ALIAS).unwrap();
        assert_eq!(entry.path, f.resource.join("tool.toml"));
        assert_eq!(entry.owner_task_id, None);
        assert_eq!(entry.description, DESCRIPTION);
        let binding = entry.binding.as_ref().unwrap();
        assert_eq!(binding["kind"], "skill");
        assert_eq!(binding["skill_md"], registered["skill_md"]);
        assert_eq!(binding["files"], registered["files"]);
        assert_eq!(binding.as_object().unwrap().len(), 3);
        let paths = json!([
            "SKILL.md",
            "assets/sample.bin",
            "references/guide.md",
            "scripts/example.sh"
        ]);
        assert_eq!(entry.input_schema["properties"]["path"]["enum"], paths);
        assert_eq!(
            entry.input_schema["properties"]["path"]["default"],
            "SKILL.md"
        );
        assert_eq!(entry.input_schema["additionalProperties"], false);
        assert!(entry.input_schema.get("required").is_none());
        assert!(!serde_json::to_string(binding)
            .unwrap()
            .contains("Host instructions"));

        assert_eq!(f.success("instructions", ALIAS, json!({})), instructions);
        assert_eq!(
            f.success("reference", ALIAS, json!({"path":"references/guide.md"})),
            REFERENCE
        );
        // The binding stores locations/inventory: reads reopen current files.
        let updated = "更新后的资料：fresh from disk, never cached.\n";
        std::fs::write(skill.join("references/guide.md"), updated).unwrap();
        assert_eq!(
            f.success(
                "fresh-reference",
                ALIAS,
                json!({"path":"references/guide.md"})
            ),
            updated
        );
        assert_eq!(
            f.success("script-text", ALIAS, json!({"path":"scripts/example.sh"})),
            script
        );
        assert!(!marker.exists(), "reading a script executed it");
        let binary_result: Value =
            serde_json::from_str(&f.success("binary", ALIAS, json!({"path":"assets/sample.bin"})))
                .unwrap();
        assert_eq!(binary_result["kind"], "binary");
        assert_eq!(binary_result["relative_path"], "assets/sample.bin");
        assert_eq!(
            binary_result["path"],
            json!(skill.join("assets/sample.bin").canonicalize().unwrap())
        );
        assert_eq!(binary_result["size_bytes"], binary.len());
        assert!(binary_result.get("content").is_none());
        for (id, path) in [
            ("unknown-file", "references/not-registered.md"),
            ("traversal", "../outside.md"),
        ] {
            let reply = f.execute(id, ALIAS, json!({"path":path}));
            assert_eq!(
                f.response(reply).unwrap_err().kind,
                ToolErrorKind::InvalidArguments
            );
            f.drained();
        }
        let reference = skill.join("references/guide.md");
        assert_eq!(
            reference.canonicalize().unwrap(),
            f.root.path().join(ALIAS).join("references/guide.md")
        );
        std::fs::remove_file(&reference).unwrap();
        let deleted = f.failure(
            "deleted-reference",
            ALIAS,
            json!({"path":"references/guide.md"}),
        );
        assert!(deleted.contains("guide.md"), "{deleted}");
        assert_eq!(f.tools(), definitions);

        // Another valid skill with the same name cannot replace the original alias.
        let conflict = f.root.path().join("another-location").join(ALIAS);
        std::fs::create_dir_all(&conflict).unwrap();
        std::fs::write(
            conflict.join("SKILL.md"),
            instructions.replace(DESCRIPTION, "Conflicting replacement description"),
        )
        .unwrap();
        let reply = f.execute(
            "conflict",
            "skill_register",
            json!({"path":conflict.join("SKILL.md")}),
        );
        let conflict_error = f.response(reply).unwrap_err();
        assert!(conflict_error.message.contains(ALIAS), "{conflict_error}");
        assert!(!conflict_error.message.contains("stack traceback"));
        f.drained();
        assert_eq!(f.tools(), definitions);
        assert_eq!(f.success("after-conflict", ALIAS, json!({})), instructions);

        let invalid = f.root.path().join("invalid-skill");
        std::fs::create_dir_all(&invalid).unwrap();
        std::fs::write(invalid.join("SKILL.md"), "---\nname: invalid-skill\ndescription: valid description\nunknown-field: invalid\n---\nNever inject me.\n").unwrap();
        let invalid_error = f.failure(
            "invalid-frontmatter",
            "skill_register",
            json!({"path":invalid.join("SKILL.md")}),
        );
        assert!(invalid_error.contains("frontmatter"), "{invalid_error}");
        let missing_error = f.failure(
            "missing-skill",
            "skill_register",
            json!({"path":f.root.path().join("missing-skill/SKILL.md")}),
        );
        assert!(missing_error.contains("resolve"), "{missing_error}");
        // Schema validation failures must also leave no pending task behind.
        let reply = f.execute(
            "invalid-register-arguments",
            "skill_register",
            json!({"path":"unused", "alias":"override"}),
        );
        assert_eq!(
            f.response(reply).unwrap_err().kind,
            ToolErrorKind::InvalidArguments
        );
        f.drained();
        assert!(f.tasks().list().is_empty());
        assert_eq!(f.tools(), definitions);
        assert!(!marker.exists());
        println!(
            "host_integration: real skill registration, static binding, fresh reads, binary metadata, validation and cleanup passed"
        );
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
