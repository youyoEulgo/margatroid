use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin};
use core_plugin::{App, Entity};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use tool_resource_core::{
    ImportTool, ToolEntry, ToolResourceError, ToolResourceErrorKind as Kind, Tools,
};
use tool_resource_plugin::ToolResourcePlugin;

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "margatroid-tool-plugin-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn entry(&self, name: &str) -> PathBuf {
        let directory = self.0.join(name);
        fs::create_dir(&directory).unwrap();
        fs::write(
            directory.join("tool.toml"),
            "schema_version = 1\ndescription = 'sample'\n",
        )
        .unwrap();
        fs::write(directory.join("input.schema.json"), "{}").unwrap();
        fs::write(directory.join("main.lua"), "function configure(arguments, context) return {timeout_ms=30000} end\nfunction execute() task_return(1) end").unwrap();
        directory.join("tool.toml")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.0.parent() == Some(std::env::temp_dir().as_path())
            && self
                .0
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("margatroid-tool-plugin-")
        {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

fn app() -> App {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(ClosurePlugin)
        .add_closure_system(RuntimePlugin::UPDATE)
        .add_plugin(ToolResourcePlugin::default());
    app
}

fn import_request(
    owner: Entity,
    alias: &str,
    path: PathBuf,
) -> (ImportTool, oneshot::Receiver<Result<(), ToolResourceError>>) {
    let (sender, receiver) = oneshot::channel();
    (
        ImportTool {
            owner,
            alias: alias.into(),
            path,
            cancel: CancellationToken::new(),
            reply: Arc::new(Mutex::new(Some(sender))),
        },
        receiver,
    )
}

fn finish(
    app: &mut App,
    receiver: &mut oneshot::Receiver<Result<(), ToolResourceError>>,
) -> Result<(), ToolResourceError> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        app.tick();
        match receiver.try_recv() {
            Ok(result) => return result,
            Err(oneshot::error::TryRecvError::Empty) => {}
            Err(error) => panic!("reply disconnected: {error}"),
        }
        assert!(Instant::now() < deadline, "import reply timed out");
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn await_validation(app: &mut App) {
    // Dispatch the ordinary validation closure before waiting for its typed result.
    app.tick();
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.world().event_snapshot().normal_event_count == 0 {
        assert!(Instant::now() < deadline, "validation timed out");
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn entry(path: PathBuf) -> ToolEntry {
    ToolEntry {
        path,
        description: "old".into(),
        input_schema: serde_json::json!({}),
        owner_task_id: None,
        binding: None,
    }
}

#[test]
fn publishes_complete_definition_and_rejects_duplicate_alias() {
    let fixture = Fixture::new();
    let first = fixture.entry("first");
    let second = fixture.entry("second");
    let mut app = app();
    let owner = app.world_mut().spawn();
    let mut collection = Tools::default();
    collection
        .insert("other".into(), entry(PathBuf::from("/unopened/other")))
        .unwrap();
    app.world_mut().insert_component(owner, collection);
    let (request, mut reply) = import_request(owner, "sample", first.clone());
    app.world().emit_event(request);
    finish(&mut app, &mut reply).unwrap();
    let recorded = app
        .world()
        .get_component::<Tools>(owner)
        .unwrap()
        .get("sample")
        .unwrap()
        .clone();
    assert_eq!(recorded.path, first.canonicalize().unwrap());
    assert_eq!(recorded.description, "sample");
    assert_eq!(recorded.input_schema, serde_json::json!({}));
    assert_eq!(recorded.owner_task_id, None);
    assert_eq!(recorded.binding, None);
    fs::write(
        first.parent().unwrap().join("tool.toml"),
        "schema_version=1\ndescription='changed'",
    )
    .unwrap();
    fs::write(first.parent().unwrap().join("input.schema.json"), "[]").unwrap();
    fs::write(first.parent().unwrap().join("main.lua"), "changed").unwrap();
    let (request, mut reply) = import_request(owner, "sample", second);
    app.world().emit_event(request);
    assert_eq!(
        finish(&mut app, &mut reply).unwrap_err().kind,
        Kind::AliasConflict
    );
    let tools = app.world().get_component::<Tools>(owner).unwrap();
    assert_eq!(tools.get("sample"), Some(&recorded));
    assert_eq!(
        tools.get("other").unwrap().path,
        PathBuf::from("/unopened/other")
    );
}

#[test]
fn failed_validation_preserves_existing_alias_and_reports_actual_path() {
    let fixture = Fixture::new();
    let first = fixture.entry("first");
    let broken = fixture.entry("broken");
    fs::write(&broken, "invalid [").unwrap();
    let mut app = app();
    let owner = app.world_mut().spawn();
    let mut collection = Tools::default();
    collection
        .insert("sample".into(), entry(first.clone()))
        .unwrap();
    app.world_mut().insert_component(owner, collection);
    let (request, mut reply) = import_request(owner, "sample", broken);
    app.world().emit_event(request);
    let error = finish(&mut app, &mut reply).unwrap_err();
    assert_eq!(error.kind, Kind::InvalidDefinition);
    assert!(error.path.is_some());
    assert_eq!(
        app.world()
            .get_component::<Tools>(owner)
            .unwrap()
            .get("sample")
            .map(|entry| entry.path.as_path()),
        Some(first.as_path())
    );
}

#[test]
fn cancellation_after_validation_prevents_publication_and_preserves_old_path() {
    let fixture = Fixture::new();
    let old = fixture.entry("old");
    let new = fixture.entry("new");
    let mut app = app();
    let owner = app.world_mut().spawn();
    let mut collection = Tools::default();
    collection
        .insert("sample".into(), entry(old.clone()))
        .unwrap();
    app.world_mut().insert_component(owner, collection);
    let (request, mut reply) = import_request(owner, "sample", new);
    let cancel = request.cancel.clone();
    app.world().emit_event(request);
    app.tick();
    await_validation(&mut app);
    assert!(matches!(
        reply.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    cancel.cancel();
    assert_eq!(
        finish(&mut app, &mut reply).unwrap_err().kind,
        Kind::Cancelled
    );
    assert_eq!(
        app.world()
            .get_component::<Tools>(owner)
            .unwrap()
            .get("sample")
            .map(|entry| entry.path.as_path()),
        Some(old.as_path())
    );
}

#[test]
fn completed_validation_cannot_write_into_reused_entity_index() {
    let fixture = Fixture::new();
    let path = fixture.entry("sample");
    let mut app = app();
    let owner = app.world_mut().spawn();
    let (request, mut reply) = import_request(owner, "sample", path);
    app.world().emit_event(request);
    app.tick();
    await_validation(&mut app);
    app.world_mut().despawn(owner);
    let replacement = app.world_mut().spawn();
    assert_eq!(owner.index(), replacement.index());
    assert_ne!(owner.generation(), replacement.generation());
    assert_eq!(
        finish(&mut app, &mut reply).unwrap_err().kind,
        Kind::OwnerMissing
    );
    assert!(!app.world().contains_component::<Tools>(replacement));
}

#[test]
fn validates_owner_alias_and_precancelled_requests_before_worker_submission() {
    let mut app = app();
    let owner = app.world_mut().spawn();
    for alias in ["", "1sample", "a.b", "a/b", "a b", "中文"] {
        let (request, mut reply) = import_request(owner, alias, PathBuf::from("/missing"));
        app.world().emit_event(request);
        assert_eq!(
            finish(&mut app, &mut reply).unwrap_err().kind,
            Kind::InvalidAlias
        );
    }
    let (request, mut reply) = import_request(owner, "valid", PathBuf::from("/missing"));
    request.cancel.cancel();
    app.world().emit_event(request);
    assert_eq!(
        finish(&mut app, &mut reply).unwrap_err().kind,
        Kind::Cancelled
    );
    app.world_mut().despawn(owner);
    let (request, mut reply) = import_request(owner, "valid", PathBuf::from("/missing"));
    app.world().emit_event(request);
    assert_eq!(
        finish(&mut app, &mut reply).unwrap_err().kind,
        Kind::OwnerMissing
    );
}

#[test]
fn duplicated_requests_claim_the_original_reply_only_once() {
    let fixture = Fixture::new();
    let path = fixture.entry("sample");
    let mut app = app();
    let owner = app.world_mut().spawn();
    let (request, mut reply) = import_request(owner, "_sample-2", path);
    let retained = request.reply.clone();
    app.world().emit_event(request.clone());
    app.world().emit_event(request);
    finish(&mut app, &mut reply).unwrap();
    assert!(retained.lock().unwrap().is_none());
    assert_eq!(
        app.world()
            .get_component::<Tools>(owner)
            .unwrap()
            .iter()
            .count(),
        1
    );
}

#[test]
fn invalid_paths_do_not_create_collections() {
    let fixture = Fixture::new();
    let mut app = app();
    let owner = app.world_mut().spawn();
    for path in [
        PathBuf::from("relative/tool.toml"),
        fixture.0.join("missing"),
        fixture.0.clone(),
    ] {
        let (request, mut reply) = import_request(owner, "sample", path);
        app.world().emit_event(request);
        assert!(finish(&mut app, &mut reply).is_err());
        assert!(!app.world().contains_component::<Tools>(owner));
    }
}

#[test]
#[should_panic(expected = "AsyncRuntimePlugin is required")]
fn installation_requires_async_runtime() {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(ToolResourcePlugin::default());
}

#[test]
#[should_panic(expected = "already installed")]
fn duplicate_installation_is_rejected() {
    let mut app = app();
    app.add_plugin(ToolResourcePlugin::default());
}

#[test]
fn configured_budget_is_enforced_on_the_configured_schedule() {
    let fixture = Fixture::new();
    let path = fixture.entry("sample");
    let mut app = App::new();
    let limits = tool_resource_core::ToolReadLimits {
        manifest_bytes: 1,
        ..tool_resource_core::ToolReadLimits::default()
    };
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(ClosurePlugin)
        .add_closure_system(RuntimePlugin::UPDATE);
    app.add_closure_system(RuntimePlugin::POST_UPDATE);
    app.add_plugin(
        ToolResourcePlugin::new()
            .with_schedule(RuntimePlugin::POST_UPDATE)
            .with_limits(limits),
    );
    let owner = app.world_mut().spawn();
    let (request, mut reply) = import_request(owner, "sample", path);
    app.world().emit_event(request);
    assert_eq!(
        finish(&mut app, &mut reply).unwrap_err().kind,
        Kind::TooLarge
    );
    assert!(!app.world().contains_component::<Tools>(owner));
}

#[cfg(unix)]
#[test]
fn canonicalizes_a_real_entry_symlink_before_publication() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    let entry = fixture.entry("sample");
    let link = fixture.0.join("entry-link");
    symlink(&entry, &link).unwrap();
    let mut app = app();
    let owner = app.world_mut().spawn();
    let (request, mut reply) = import_request(owner, "sample", link);
    app.world().emit_event(request);
    finish(&mut app, &mut reply).unwrap();
    assert_eq!(
        app.world()
            .get_component::<Tools>(owner)
            .unwrap()
            .get("sample")
            .map(|entry| entry.path.as_path()),
        Some(entry.canonicalize().unwrap().as_path())
    );
}
