use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use async_runtime_plugin::AsyncRuntimePlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin};
use core_plugin::{App, Entity};
use hook_resource_plugin::HookResource;
use hook_resource_plugin::{HookResourcePlugin, LoadHookResource};
use margatroid_types::execution_components::{AgentExecutionScope, ExecutionActivityKind};
use margatroid_types::resource_events::HookResourceTaskError;
use margatroid_types::resource_events::{
    HookResourceReadOutput, ResourceLoadError, ResourceLoadErrorKind as Kind,
};
use resource_id_plugin::{ResourceID, ResourcePlugin, ResourceSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn fixture() -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "margatroid-hook-resource-{}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let directory = root.join("hooks/local/sample/latest");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("hook.toml"),
        "name = \"sample\"\ndescription = \"description\"\n",
    )
    .unwrap();
    fs::write(directory.join("input.schema.json"), "{\"type\":\"object\"}").unwrap();
    root
}
fn app(root: &Path) -> App {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(ClosurePlugin)
        .add_closure_system(RuntimePlugin::PRE_UPDATE)
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(ResourcePlugin)
        .add_plugin(HookResourcePlugin::open(root).unwrap());
    app
}
fn request(
    target: Entity,
    alias: &str,
) -> (
    LoadHookResource,
    tokio::sync::oneshot::Receiver<Result<(), ResourceLoadError>>,
    AgentExecutionScope,
) {
    let scope = AgentExecutionScope::new();
    let activity = Arc::new(scope.register(ExecutionActivityKind::Other).unwrap());
    let (tx, rx) = tokio::sync::oneshot::channel();
    let request = LoadHookResource {
        id: "request".into(),
        target,
        resource_id: ResourceID::parse("hook:local/sample").unwrap(),
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
        .get_component::<ResourceSet<HookResource>>(target)
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
        root.join("hooks/local/sample/latest/hook.toml"),
        "invalid definition",
    )
    .unwrap();
    assert!(load(&mut app, target, "new").is_err());
    let set = app
        .world()
        .get_component::<ResourceSet<HookResource>>(target)
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
        .get_component::<ResourceSet<HookResource>>(target)
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
        .send_event(Ok::<_, HookResourceTaskError>(HookResourceReadOutput {
            request,
            result: Ok(HookResource {
                alias: "local".into(),
                description: "prepared".into(),
                input_schema: serde_json::json!({}),
                script_path: root.join("unused.lua"),
            }),
        }));
    assert_eq!(wait(&mut app, rx).unwrap_err().kind, Kind::Cancelled);
    assert!(app
        .world()
        .get_component::<ResourceSet<HookResource>>(target)
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
        .send_event(Ok::<_, HookResourceTaskError>(HookResourceReadOutput {
            request,
            result: Ok(HookResource {
                alias: "local".into(),
                description: "prepared".into(),
                input_schema: serde_json::json!({}),
                script_path: root.join("unused.lua"),
            }),
        }));
    assert_eq!(wait(&mut app, rx).unwrap_err().kind, Kind::TargetNotAlive);
    assert!(app
        .world()
        .get_component::<ResourceSet<HookResource>>(reused)
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
            .get_component::<ResourceSet<HookResource>>(target)
            .unwrap();
        assert_eq!(set.iter().next().unwrap().1.alias, alias);
    }
}
#[test]
fn rejects_relative_root_and_parent_traversal() {
    assert!(HookResourcePlugin::open("relative").is_err());
    assert!(HookResourcePlugin::open("/tmp/../escape").is_err());
}
#[cfg(unix)]
#[test]
fn necessary_file_symlink_is_rejected() {
    let root = fixture();
    let directory = root.join("hooks/local/sample/latest");
    let path = directory.join("hook.toml");
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
    let task = HookResourceTaskError {
        request: Some(request),
        message: "injected supervision failure".into(),
    };
    app.world()
        .send_event(Err::<HookResourceReadOutput, _>(task));
    assert_eq!(wait(&mut app, rx).unwrap_err().kind, Kind::TaskFailed);
    let set = app
        .world()
        .get_component::<ResourceSet<HookResource>>(target)
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
        .get_component::<ResourceSet<HookResource>>(target)
        .unwrap()
        .iter()
        .next()
        .unwrap()
        .1
        .clone();
    let mut updated = existing;
    updated.alias = "should not replace".into();
    app.world()
        .send_event(Ok::<_, HookResourceTaskError>(HookResourceReadOutput {
            request: duplicate,
            result: Ok(updated),
        }));
    app.tick();
    assert_eq!(
        app.world()
            .get_component::<ResourceSet<HookResource>>(target)
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
    let mut limits = hook_resource_plugin::HookResourceLoadLimits::default();
    limits.max_definition_bytes = 1;
    limits.max_content_bytes = 1;
    limits.max_policy_bytes = 1;
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(ClosurePlugin)
        .add_closure_system(RuntimePlugin::PRE_UPDATE)
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(ResourcePlugin)
        .add_plugin(HookResourcePlugin::open(&root).unwrap().with_limits(limits));
    let target = app.world_mut().spawn();
    assert_eq!(
        load(&mut app, target, "alias").unwrap_err().kind,
        Kind::LimitExceeded
    );
    assert!(app
        .world()
        .get_component::<ResourceSet<HookResource>>(target)
        .is_none());
}

#[test]
fn hook_ignores_lua_and_other_unowned_files() {
    let root = fixture();
    fs::write(root.join("hooks/local/sample/latest/main.lua"), [255u8]).unwrap();
    let mut app = app(&root);
    let target = app.world_mut().spawn();
    load(&mut app, target, "trigger").unwrap();
}

#[test]
fn invalid_schema_and_external_references_are_rejected() {
    for schema in [
        "{\"type\":123}",
        "{\"$ref\":\"https://example.invalid/schema\"}",
    ] {
        let root = fixture();
        fs::write(
            root.join("hooks/local/sample/latest/input.schema.json"),
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
    let second_id = ResourceID::parse("hook:local/second").unwrap();
    let set = app
        .world_mut()
        .get_component_mut::<ResourceSet<HookResource>>(target)
        .unwrap();
    let mut second = set.iter().next().unwrap().1.clone();
    second.alias = "untouched".into();
    set.insert(second_id.clone(), second);
    load(&mut app, target, "updated").unwrap();
    let set = app
        .world()
        .get_component::<ResourceSet<HookResource>>(target)
        .unwrap();
    assert_eq!(set.len(), 2);
    assert_eq!(set.get(&second_id).unwrap().alias, "untouched");
}
#[test]
fn invalid_utf8_cannot_mount() {
    let root = fixture();
    fs::write(root.join("hooks/local/sample/latest/hook.toml"), [255u8]).unwrap();
    let mut app = app(&root);
    let target = app.world_mut().spawn();
    assert_eq!(
        load(&mut app, target, "alias").unwrap_err().kind,
        Kind::DecodeFailed
    );
    assert!(app
        .world()
        .get_component::<ResourceSet<HookResource>>(target)
        .is_none());
}
