mod support;
use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use async_runtime_plugin::AsyncRuntimePlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin};
use core_plugin::{App, Entity};
use margatroid_types::agent_components::{Agent, WorldAgentExt};
use margatroid_types::block_storage::{
    BlockExpression, BlockRange, BlockSchema, BlockSelector, BlockSource, BlockStorageContext,
    BlockStorageOperation, BlockStorageRequest, StoredBlockInner, StoredBlockResult,
};
use margatroid_types::block_types::{BlockInner, BlockPath, InnerType};
use margatroid_types::execution_components::{AgentExecutionScope, ExecutionActivityKind};
use margatroid_types::resource_events::ToolResourceTaskError;
use margatroid_types::resource_events::{
    ResourceLoadError, ResourceLoadErrorKind as Kind, ToolResourceReadOutput,
};
use resource_id_plugin::{ResourceID, ResourcePlugin, ResourceSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tool_resource_plugin::ToolResource;
use tool_resource_plugin::{LoadToolResource, ToolResourcePlugin};

fn fixture() -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "margatroid-tool-resource-{}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let directory = root.join("tools/local/sample/latest");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("tool.toml"),
        "schema_version = 1\nname = \"sample\"\ndescription = \"description\"\n",
    )
    .unwrap();
    fs::write(directory.join("input.schema.json"), "{\"type\":\"object\"}").unwrap();
    fs::write(directory.join("main.lua"), "return{}").unwrap();
    root
}
fn app(root: &Path) -> App {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(ClosurePlugin)
        .add_closure_system(RuntimePlugin::PRE_UPDATE)
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(ResourcePlugin)
        .add_plugin(ToolResourcePlugin::open(root).unwrap());
    app
}
fn request(
    target: Entity,
    alias: &str,
) -> (
    LoadToolResource,
    tokio::sync::oneshot::Receiver<Result<(), ResourceLoadError>>,
    AgentExecutionScope,
) {
    let scope = AgentExecutionScope::new();
    let activity = Arc::new(scope.register(ExecutionActivityKind::Other).unwrap());
    let (tx, rx) = tokio::sync::oneshot::channel();
    let request = LoadToolResource {
        id: "request".into(),
        target,
        resource_id: ResourceID::parse("tool:local/sample").unwrap(),
        alias: alias.into(),
        import_kind: None,
        cancel: activity.cancel.clone(),
        reply: Arc::new(Mutex::new(Some(tx))),
        activity: Some(activity),
    };
    (request, rx, scope)
}
fn wait(
    app: &mut App,
    mut rx: tokio::sync::oneshot::Receiver<Result<(), ResourceLoadError>>,
) -> Result<(), ResourceLoadError> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        app.tick();
        match rx.try_recv() {
            Ok(result) => return result,
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                panic!("reply channel closed")
            }
            Err(_) => {}
        }
        assert!(Instant::now() < deadline, "resource read timed out");
        std::thread::yield_now();
    }
}
fn load(app: &mut App, target: Entity, alias: &str) -> Result<(), ResourceLoadError> {
    let (request, rx, scope) = request(target, alias);
    app.world().send_event(request);
    let result = wait(app, rx);
    assert_eq!(scope.activity_count(), 0);
    result
}
#[test]
fn loads_and_reload_updates_alias_without_adding_keys() {
    let root = fixture();
    let mut app = app(&root);
    let target = app.world_mut().spawn();
    load(&mut app, target, "first").unwrap();
    load(&mut app, target, "second").unwrap();
    let set = app
        .world()
        .get_component::<ResourceSet<ToolResource>>(target)
        .unwrap();
    assert_eq!(set.len(), 1);
    assert_eq!(set.iter().next().unwrap().1.alias, "second");
}
#[test]
fn failed_reload_preserves_old_entry_and_alias() {
    let root = fixture();
    let mut app = app(&root);
    let target = app.world_mut().spawn();
    load(&mut app, target, "old").unwrap();
    fs::write(
        root.join("tools/local/sample/latest/tool.toml"),
        "invalid definition",
    )
    .unwrap();
    assert!(load(&mut app, target, "new").is_err());
    let set = app
        .world()
        .get_component::<ResourceSet<ToolResource>>(target)
        .unwrap();
    assert_eq!(set.len(), 1);
    assert_eq!(set.iter().next().unwrap().1.alias, "old");
}
#[test]
fn cancelled_request_does_not_mount() {
    let root = fixture();
    let mut app = app(&root);
    let target = app.world_mut().spawn();
    let (request, rx, scope) = request(target, "local");
    request.cancel.cancel();
    app.world().send_event(request);
    assert_eq!(wait(&mut app, rx).unwrap_err().kind, Kind::Cancelled);
    assert!(app
        .world()
        .get_component::<ResourceSet<ToolResource>>(target)
        .is_none());
    assert_eq!(scope.activity_count(), 0);
}
#[test]
fn cancellation_after_preparation_does_not_mount() {
    let root = fixture();
    let mut app = app(&root);
    let target = app.world_mut().spawn();
    let (request, rx, scope) = request(target, "local");
    request.activity.as_ref().unwrap().mark_started().unwrap();
    request.cancel.cancel();
    app.world()
        .send_event(Ok::<_, ToolResourceTaskError>(ToolResourceReadOutput {
            request,
            result: Ok(ToolResource {
                alias: "local".into(),
                description: "prepared".into(),
                input_schema: serde_json::json!({}),
                script_path: root.join("unused.lua"),
            }),
        }));
    assert_eq!(wait(&mut app, rx).unwrap_err().kind, Kind::Cancelled);
    assert!(app
        .world()
        .get_component::<ResourceSet<ToolResource>>(target)
        .is_none());
    assert_eq!(scope.activity_count(), 0);
}
#[test]
fn late_output_cannot_mount_into_reused_target_index() {
    let root = fixture();
    let mut app = app(&root);
    let target = app.world_mut().spawn();
    let (request, rx, scope) = request(target, "local");
    request.activity.as_ref().unwrap().mark_started().unwrap();
    app.world_mut().despawn(target);
    let reused = app.world_mut().spawn();
    assert_eq!(target.index(), reused.index());
    assert_ne!(target.generation(), reused.generation());
    app.world()
        .send_event(Ok::<_, ToolResourceTaskError>(ToolResourceReadOutput {
            request,
            result: Ok(ToolResource {
                alias: "local".into(),
                description: "prepared".into(),
                input_schema: serde_json::json!({}),
                script_path: root.join("unused.lua"),
            }),
        }));
    assert_eq!(wait(&mut app, rx).unwrap_err().kind, Kind::TargetNotAlive);
    assert!(app
        .world()
        .get_component::<ResourceSet<ToolResource>>(reused)
        .is_none());
    assert_eq!(scope.activity_count(), 0);
}
#[test]
fn target_entities_have_independent_aliases() {
    let root = fixture();
    let mut app = app(&root);
    let first = app.world_mut().spawn();
    let second = app.world_mut().spawn();
    load(&mut app, first, "first").unwrap();
    load(&mut app, second, "second").unwrap();
    for (target, alias) in [(first, "first"), (second, "second")] {
        let set = app
            .world()
            .get_component::<ResourceSet<ToolResource>>(target)
            .unwrap();
        assert_eq!(set.iter().next().unwrap().1.alias, alias);
    }
}
#[test]
fn rejects_relative_root_and_parent_traversal() {
    assert!(ToolResourcePlugin::open("relative").is_err());
    assert!(ToolResourcePlugin::open("/tmp/../escape").is_err());
}
#[cfg(unix)]
#[test]
fn necessary_file_symlink_is_rejected() {
    let root = fixture();
    let directory = root.join("tools/local/sample/latest");
    let path = directory.join("tool.toml");
    let saved = directory.join("original");
    fs::rename(&path, &saved).unwrap();
    std::os::unix::fs::symlink(&saved, &path).unwrap();
    let mut app = app(&root);
    let target = app.world_mut().spawn();
    assert_eq!(
        load(&mut app, target, "alias").unwrap_err().kind,
        Kind::SymlinkNotAllowed
    );
}

#[test]
fn task_failure_releases_activity_and_preserves_resources() {
    let root = fixture();
    let mut app = app(&root);
    let target = app.world_mut().spawn();
    load(&mut app, target, "old").unwrap();
    let (request, rx, scope) = request(target, "new");
    request.activity.as_ref().unwrap().mark_started().unwrap();
    let task = ToolResourceTaskError {
        request: Some(request),
        message: "injected supervision failure".into(),
    };
    app.world()
        .send_event(Err::<ToolResourceReadOutput, _>(task));
    assert_eq!(wait(&mut app, rx).unwrap_err().kind, Kind::TaskFailed);
    let set = app
        .world()
        .get_component::<ResourceSet<ToolResource>>(target)
        .unwrap();
    assert_eq!(set.iter().next().unwrap().1.alias, "old");
    assert_eq!(scope.activity_count(), 0);
}
#[test]
fn repeated_result_does_not_mount_after_reply_was_taken() {
    let root = fixture();
    let mut app = app(&root);
    let target = app.world_mut().spawn();
    let (request, rx, scope) = request(target, "first");
    let duplicate = request.clone();
    app.world().send_event(request);
    wait(&mut app, rx).unwrap();
    assert_eq!(scope.activity_count(), 0);
    let existing = app
        .world()
        .get_component::<ResourceSet<ToolResource>>(target)
        .unwrap()
        .iter()
        .next()
        .unwrap()
        .1
        .clone();
    let mut updated = existing;
    updated.alias = "should not replace".into();
    app.world()
        .send_event(Ok::<_, ToolResourceTaskError>(ToolResourceReadOutput {
            request: duplicate,
            result: Ok(updated),
        }));
    app.tick();
    assert_eq!(
        app.world()
            .get_component::<ResourceSet<ToolResource>>(target)
            .unwrap()
            .iter()
            .next()
            .unwrap()
            .1
            .alias,
        "first"
    );
}
#[test]
fn byte_limits_reject_load_without_mount() {
    let root = fixture();
    let mut app = App::new();
    let mut limits = tool_resource_plugin::ToolResourceLoadLimits::default();
    limits.max_definition_bytes = 1;
    limits.max_content_bytes = 1;
    limits.max_policy_bytes = 1;
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(ClosurePlugin)
        .add_closure_system(RuntimePlugin::PRE_UPDATE)
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(ResourcePlugin)
        .add_plugin(ToolResourcePlugin::open(&root).unwrap().with_limits(limits));
    let target = app.world_mut().spawn();
    assert_eq!(
        load(&mut app, target, "alias").unwrap_err().kind,
        Kind::LimitExceeded
    );
    assert!(app
        .world()
        .get_component::<ResourceSet<ToolResource>>(target)
        .is_none());
}

#[test]
fn invalid_schema_and_external_references_are_rejected() {
    for schema in [
        "{\"type\":123}",
        "{\"$ref\":\"https://example.invalid/schema\"}",
    ] {
        let root = fixture();
        fs::write(
            root.join("tools/local/sample/latest/input.schema.json"),
            schema,
        )
        .unwrap();
        let mut app = app(&root);
        let target = app.world_mut().spawn();
        assert_eq!(
            load(&mut app, target, "alias").unwrap_err().kind,
            Kind::InvalidDefinition
        );
    }
}

#[test]
fn reload_preserves_other_resource_keys() {
    let root = fixture();
    let mut app = app(&root);
    let target = app.world_mut().spawn();
    load(&mut app, target, "old").unwrap();
    let second_id = ResourceID::parse("tool:local/second").unwrap();
    let set = app
        .world_mut()
        .get_component_mut::<ResourceSet<ToolResource>>(target)
        .unwrap();
    let mut second = set.iter().next().unwrap().1.clone();
    second.alias = "untouched".into();
    set.insert(second_id.clone(), second);
    load(&mut app, target, "updated").unwrap();
    let set = app
        .world()
        .get_component::<ResourceSet<ToolResource>>(target)
        .unwrap();
    assert_eq!(set.len(), 2);
    assert_eq!(set.get(&second_id).unwrap().alias, "untouched");
}
#[test]
fn invalid_utf8_cannot_mount() {
    let root = fixture();
    fs::write(root.join("tools/local/sample/latest/main.lua"), [255u8]).unwrap();
    let mut app = app(&root);
    let target = app.world_mut().spawn();
    assert_eq!(
        load(&mut app, target, "alias").unwrap_err().kind,
        Kind::DecodeFailed
    );
    assert!(app
        .world()
        .get_component::<ResourceSet<ToolResource>>(target)
        .is_none());
}

fn import(
    app: &mut App,
    target: Entity,
    alias: &str,
    kind: InnerType,
) -> Result<(), ResourceLoadError> {
    let (mut request, rx, scope) = request(target, alias);
    request.import_kind = Some(kind);
    app.world().send_event(request);
    let result = wait(app, rx);
    assert_eq!(scope.activity_count(), 0);
    result
}
#[tokio::test]
async fn typed_import_reload_rename_preserves_block_snapshot_and_failed_reload() {
    let root = fixture();
    let mut app = app(&root);
    let (target, id) = support::agent(&mut app, &["tool:local/sample"]);
    import(&mut app, target, "old", InnerType::Tool).unwrap();
    let old = app.world().agent_get_import(&id, "old").unwrap();
    assert!(matches!(&old, BlockInner::Tool(v) if v.len() == 1 && v[0].alias == "old"));
    let storage = app.world().agent_block_storage(&id).unwrap();
    let context = BlockStorageContext {
        agent: id.clone(),
        instance: margatroid_types::lua_runtime_components::DriverLuaVmInstance {
            owner: target,
            start_request_id: "fixture".into(),
        },
    };
    storage
        .execute(BlockStorageRequest {
            context: context.clone(),
            operation: BlockStorageOperation::Create(BlockSchema {
                name: "saved".into(),
                fields: vec![("tools".into(), InnerType::Tool)],
                temporary: false,
            }),
            cancel: Default::default(),
        })
        .await
        .unwrap();
    let path = BlockPath {
        block_id: "saved".into(),
        inner_id: "tools".into(),
    };
    let stored = StoredBlockInner::from(old.clone());
    storage
        .execute(BlockStorageRequest {
            context: context.clone(),
            operation: BlockStorageOperation::Inject {
                source: BlockExpression::Sources(vec![BlockSource::Values(stored.clone())]),
                target: BlockSelector {
                    path: path.clone(),
                    range: BlockRange::Empty,
                },
            },
            cancel: Default::default(),
        })
        .await
        .unwrap();
    fs::write(
        root.join("tools/local/sample/latest/tool.toml"),
        "schema_version=1\nname=\"sample\"\ndescription=\"new description\"\n",
    )
    .unwrap();
    import(&mut app, target, "renamed", InnerType::Tool).unwrap();
    assert!(app.world().agent_get_import(&id, "old").is_err());
    assert_eq!(
        storage
            .execute(BlockStorageRequest {
                context,
                operation: BlockStorageOperation::Get {
                    expressions: vec![BlockExpression::Sources(vec![BlockSource::Selector(
                        BlockSelector {
                            path,
                            range: BlockRange::All
                        },
                    )])],
                },
                cancel: Default::default(),
            })
            .await
            .unwrap(),
        StoredBlockResult::Arrays(vec![stored]),
    );
    let updated = app.world().agent_get_import(&id, "renamed").unwrap();
    assert_ne!(updated, old);
    assert_eq!(
        app.world()
            .get_component::<ResourceSet<ToolResource>>(target)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        import(&mut app, target, "renamed", InnerType::Message)
            .unwrap_err()
            .kind,
        Kind::InvalidRequest
    );
    fs::write(root.join("tools/local/sample/latest/tool.toml"), "bad").unwrap();
    assert!(import(&mut app, target, "replacement", InnerType::Tool).is_err());
    assert_eq!(
        app.world().agent_get_import(&id, "renamed").unwrap(),
        updated
    );
    assert_eq!(
        app.world()
            .get_component::<ResourceSet<ToolResource>>(target)
            .unwrap()
            .iter()
            .next()
            .unwrap()
            .1
            .alias,
        "renamed"
    );
}
#[test]
fn alias_conflict_and_missing_dependency_preserve_old_bindings_and_set() {
    use margatroid_types::agent_components::ImportedValue;
    let root = fixture();
    let mut app = app(&root);
    let (target, id) = support::agent(&mut app, &["tool:local/sample", "prompt:local/other"]);
    import(&mut app, target, "old", InnerType::Tool).unwrap();
    let other = ImportedValue::Message {
        resource_id: ResourceID::parse("prompt:local/other").unwrap(),
        messages: Arc::from([]),
    };
    app.world_mut()
        .agent_bind_import(&id, "occupied".into(), other)
        .unwrap();
    let before = app
        .world()
        .get_component::<Agent>(target)
        .unwrap()
        .import_bindings
        .clone();
    assert!(import(&mut app, target, "occupied", InnerType::Tool).is_err());
    assert_eq!(
        app.world()
            .get_component::<Agent>(target)
            .unwrap()
            .import_bindings,
        before
    );
    assert_eq!(
        app.world()
            .get_component::<ResourceSet<ToolResource>>(target)
            .unwrap()
            .iter()
            .next()
            .unwrap()
            .1
            .alias,
        "old"
    );
    app.world_mut()
        .get_component_mut::<Agent>(target)
        .unwrap()
        .info
        .driver_dependencies = Arc::from([]);
    assert!(import(&mut app, target, "new", InnerType::Tool).is_err());
    assert_eq!(
        app.world()
            .get_component::<Agent>(target)
            .unwrap()
            .import_bindings,
        before
    );
}
#[test]
fn cancelled_typed_output_does_not_bind_and_plain_agent_load_does_not_bind() {
    let root = fixture();
    let mut app = app(&root);
    let (target, id) = support::agent(&mut app, &["tool:local/sample"]);
    load(&mut app, target, "plain").unwrap();
    assert!(app.world().agent_get_import(&id, "plain").is_err());
    let (mut request, rx, scope) = request(target, "cancelled");
    request.import_kind = Some(InnerType::Tool);
    request.activity.as_ref().unwrap().mark_started().unwrap();
    request.cancel.cancel();
    let resource = app
        .world()
        .get_component::<ResourceSet<ToolResource>>(target)
        .unwrap()
        .iter()
        .next()
        .unwrap()
        .1
        .clone();
    app.world()
        .send_event(Ok::<_, ToolResourceTaskError>(ToolResourceReadOutput {
            request,
            result: Ok(resource),
        }));
    assert_eq!(wait(&mut app, rx).unwrap_err().kind, Kind::Cancelled);
    assert!(app.world().agent_get_import(&id, "cancelled").is_err());
    assert_eq!(scope.activity_count(), 0);
    assert_eq!(
        app.world()
            .get_component::<ResourceSet<ToolResource>>(target)
            .unwrap()
            .iter()
            .next()
            .unwrap()
            .1
            .alias,
        "plain"
    );
}

#[test]
fn invalid_existing_set_address_rejects_before_binding_and_keeps_exact_state() {
    let root = fixture();
    let mut app = app(&root);
    let (target, id) = support::agent(&mut app, &["tool:local/sample"]);
    import(&mut app, target, "old", InnerType::Tool).unwrap();
    let before = app
        .world()
        .get_component::<Agent>(target)
        .unwrap()
        .import_bindings
        .clone();
    let invalid_id = ResourceID::parse("prompt:local/wrong").unwrap();
    let set = app
        .world_mut()
        .get_component_mut::<ResourceSet<ToolResource>>(target)
        .unwrap();
    let resource = set.iter().next().unwrap().1.clone();
    set.insert(invalid_id.clone(), resource);
    assert_eq!(
        import(&mut app, target, "new", InnerType::Tool)
            .unwrap_err()
            .kind,
        Kind::MountFailed
    );
    assert_eq!(
        app.world()
            .get_component::<Agent>(target)
            .unwrap()
            .import_bindings,
        before
    );
    let set = app
        .world()
        .get_component::<ResourceSet<ToolResource>>(target)
        .unwrap();
    assert_eq!(set.len(), 2);
    assert_eq!(set.get(&invalid_id).unwrap().alias, "old");
    assert_eq!(
        set.get(&ResourceID::parse("tool:local/sample").unwrap())
            .unwrap()
            .alias,
        "old"
    );
    assert!(app.world().agent_get_import(&id, "new").is_err());
}

#[test]
fn missing_dependency_directory_preserves_chain_resource_path_and_cause() {
    let root = fixture();
    let mut app = app(&root);
    let target = app.world_mut().spawn();
    let (mut request, rx, scope) = request(target, "dependency");
    let chain = format!(
        "driver:local/coder:latest -> {} -> dependency",
        "parent".repeat(100)
    );
    request.id = chain.clone();
    request.resource_id = ResourceID::parse("tool:local/absent:latest").unwrap();
    app.world().send_event(request);
    let error = wait(&mut app, rx).unwrap_err();
    assert_eq!(error.kind(), Kind::NotFound);
    assert!(error.message.contains(&chain), "{error}");
    assert!(
        error.message.contains("tool:local/absent:latest"),
        "{error}"
    );
    let path = root.join("tools/local/absent/latest");
    assert!(error.message.contains(path.to_str().unwrap()), "{error}");
    let ancestor = root.join("tools/local/absent");
    let cause = fs::symlink_metadata(&ancestor).unwrap_err().to_string();
    assert!(
        error.message.contains(ancestor.to_str().unwrap()),
        "{error}"
    );
    assert!(error.message.contains(&cause), "{error}");
    assert_eq!(scope.activity_count(), 0);
}

#[test]
fn missing_dependency_files_report_exact_path_and_preserve_old_entry() {
    for filename in ["tool.toml", "input.schema.json", "main.lua"] {
        let root = fixture();
        let mut app = app(&root);
        let target = app.world_mut().spawn();
        load(&mut app, target, "old").unwrap();
        let path = root.join("tools/local/sample/latest").join(filename);
        assert!(path.is_absolute() && path.starts_with(&root));
        fs::remove_file(&path).unwrap();
        let error = load(&mut app, target, "new").unwrap_err();
        assert_eq!(error.kind(), Kind::NotFound);
        assert!(
            error.message.contains("tool:local/sample:latest"),
            "{error}"
        );
        assert!(error.message.contains(path.to_str().unwrap()), "{error}");
        let cause = fs::symlink_metadata(&path).unwrap_err().to_string();
        assert!(error.message.contains(&cause), "{error}");
        let set = app
            .world()
            .get_component::<ResourceSet<ToolResource>>(target)
            .unwrap();
        assert_eq!(set.iter().next().unwrap().1.alias, "old");
    }
}

#[test]
fn malformed_definition_preserves_parser_cause_and_absolute_file() {
    let root = fixture();
    let path = root.join("tools/local/sample/latest/tool.toml");
    let body = "{ invalid definition";
    fs::write(&path, body).unwrap();
    let cause = toml::from_str::<toml::Value>(body).unwrap_err().to_string();
    let mut app = app(&root);
    let target = app.world_mut().spawn();
    let error = load(&mut app, target, "invalid").unwrap_err();
    assert_eq!(error.kind(), Kind::InvalidDefinition);
    assert!(error.message.contains(path.to_str().unwrap()), "{error}");
    assert!(error.message.contains(&cause), "{error}");
}
