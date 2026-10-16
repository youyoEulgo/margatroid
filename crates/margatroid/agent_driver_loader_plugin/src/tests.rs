use super::*;
use app_runtime_plugin::WorldEventExt;
use async_runtime_plugin::AsyncRuntimePlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin};
use core_plugin::Entity;
use margatroid_types::agent_driver_events::AgentDriverReadOutput;
use resource_id_plugin::{ResourceID, ResourcePlugin};
use std::fs;
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
static COUNTER: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "margatroid-driver-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let driver = root.join("local/coder/latest");
        fs::create_dir_all(&driver).unwrap();
        fs::write(driver.join("driver.toml"), "schema_version = 1\n[inference]\nmodel = 'test'\ntemperature = 0.7\nmax_output_tokens = 100\ntop_p = 0.9\nstop = ['DONE']\n[[dependencies]]\nid = 'prompt:system/test:v1'\n").unwrap();
        fs::write(driver.join("base.lua"), "return 'original'").unwrap();
        Self(root)
    }
    fn driver(&self) -> PathBuf {
        self.0.join("local/coder/latest")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        assert!(
            self.0.is_absolute()
                && self
                    .0
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("margatroid-driver-")
        );
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn app(root: &Path) -> App {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(ClosurePlugin)
        .add_closure_system(RuntimePlugin::PRE_UPDATE)
        .add_plugin(ResourcePlugin)
        .add_plugin(AgentDriverPlugin::open(root).unwrap());
    app
}
fn request(id: &str, target: Entity) -> LoadAgentDriver {
    LoadAgentDriver {
        id: id.into(),
        target,
        resource_id: ResourceID::parse("driver:local/coder").unwrap(),
    }
}
fn wait(app: &mut App, ids: &[&str]) -> Vec<LoadAgentDriverResult> {
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut found = Vec::new();
    while found.len() < ids.len() {
        app.tick();
        for event in app.world().event_reader::<LoadAgentDriverResult>() {
            if ids.contains(&event.id.as_str())
                && !found
                    .iter()
                    .any(|e: &LoadAgentDriverResult| e.id == event.id)
            {
                found.push(event.clone());
            }
        }
        assert!(Instant::now() < deadline, "load timed out");
        std::thread::yield_now();
    }
    found
}
fn load(app: &mut App, id: &str, target: Entity) -> Result<(), DriverLoadError> {
    app.world().send_event(request(id, target));
    wait(app, &[id]).pop().unwrap().result
}
fn driver_source(app: &App, target: Entity) -> String {
    app.world()
        .get_component::<ResourceSet<AgentDriver>>(target)
        .unwrap()
        .get(&request("x", target).resource_id)
        .unwrap()
        .base_driver
        .to_string()
}
#[test]
fn mounts_real_target_and_keeps_other_keys() {
    let fixture = Fixture::new();
    let mut app = app(&fixture.0);
    let target = app.world_mut().spawn();
    load(&mut app, "first", target).unwrap();
    let set = app
        .world()
        .get_component::<ResourceSet<AgentDriver>>(target)
        .unwrap();
    let driver = set.get(&request("x", target).resource_id).unwrap().clone();
    assert_eq!(driver.model.model, "test");
    assert_eq!(driver.model.temperature, Some(0.7));
    assert_eq!(driver.dependencies[0].to_string(), "prompt:system/test:v1");
    assert_eq!(driver.origin.as_path(), fixture.driver().join("base.lua"));
    let other = ResourceID::parse("driver:local/other").unwrap();
    app.world_mut()
        .get_component_mut::<ResourceSet<AgentDriver>>(target)
        .unwrap()
        .insert(other.clone(), driver);
    fs::write(fixture.driver().join("base.lua"), "updated").unwrap();
    load(&mut app, "reload", target).unwrap();
    assert_eq!(driver_source(&app, target), "updated");
    assert!(app
        .world()
        .get_component::<ResourceSet<AgentDriver>>(target)
        .unwrap()
        .get(&other)
        .is_some());
    assert_eq!(app.world().entity_count(), 1);
}
#[test]
fn same_id_multiple_targets_have_independent_mounts() {
    let fixture = Fixture::new();
    let mut app = app(&fixture.0);
    let first = app.world_mut().spawn();
    let second = app.world_mut().spawn();
    app.world().send_event(request("first", first));
    app.world().send_event(request("second", second));
    let replies = wait(&mut app, &["first", "second"]);
    assert!(replies.iter().all(|r| r.result.is_ok()));
    assert_eq!(driver_source(&app, first), driver_source(&app, second));
    fs::write(fixture.driver().join("base.lua"), "new").unwrap();
    load(&mut app, "reload", first).unwrap();
    assert_eq!(driver_source(&app, first), "new");
    assert_eq!(driver_source(&app, second), "return 'original'");
    assert_eq!(app.world().entity_count(), 2);
}
#[test]
fn failed_reload_preserves_previous_key() {
    let fixture = Fixture::new();
    let mut app = app(&fixture.0);
    let target = app.world_mut().spawn();
    load(&mut app, "first", target).unwrap();
    fs::write(fixture.driver().join("base.lua"), [0xff]).unwrap();
    assert!(load(&mut app, "broken", target).is_err());
    assert_eq!(driver_source(&app, target), "return 'original'");
}
#[test]
fn stale_generation_cannot_mount_reused_target() {
    let fixture = Fixture::new();
    let mut app = app(&fixture.0);
    let target = app.world_mut().spawn();
    load(&mut app, "first", target).unwrap();
    let driver = app
        .world()
        .get_component::<ResourceSet<AgentDriver>>(target)
        .unwrap()
        .get(&request("x", target).resource_id)
        .unwrap()
        .clone();
    assert!(app.world_mut().despawn(target));
    let replacement = app.world_mut().spawn();
    assert_eq!(target.index(), replacement.index());
    assert_ne!(target, replacement);
    let completed = Arc::new(AtomicBool::new(false));
    handler::handle_mount_agent_driver(
        app.world_mut(),
        AgentDriverReadOutput {
            request: request("stale", target),
            result: Ok(driver),
            completed: completed.clone(),
            supervision: Arc::new(()),
        },
    );
    assert!(completed.load(Ordering::Acquire));
    assert!(app
        .world()
        .get_component::<ResourceSet<AgentDriver>>(replacement)
        .is_none());
    assert!(wait(&mut app, &["stale"])[0].result.is_err());
}
#[test]
fn ignores_unowned_files_and_directories() {
    let fixture = Fixture::new();
    for i in 0..40 {
        fs::write(fixture.driver().join(format!("unowned{i}")), [0xff]).unwrap();
    }
    fs::create_dir(fixture.driver().join("skills")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("missing", fixture.driver().join("unowned-link")).unwrap();
    let mut app = app(&fixture.0);
    let target = app.world_mut().spawn();
    load(&mut app, "ignore", target).unwrap();
}
#[test]
fn rejects_dependency_source_and_invalid_model_parameters() {
    for body in ["schema_version=1\n[inference]\nmodel='test'\n[[dependencies]]\nid='prompt:local/a'\nsource='body'", "schema_version=1\n[inference]\nmodel='test'\ntemperature=-1", "schema_version=1\n[inference]\nmodel='test'\n[[dependencies]]\nid='../escape'"] {
        let fixture = Fixture::new();
        fs::write(fixture.driver().join("driver.toml"), body).unwrap();
        let mut app = app(&fixture.0);
        let target = app.world_mut().spawn();
        assert!(load(&mut app, "invalid", target).is_err());
        assert!(app.world().get_component::<ResourceSet<AgentDriver>>(target).is_none());
    }
}
#[test]
fn both_files_are_bounded() {
    for name in ["driver.toml", "base.lua"] {
        let fixture = Fixture::new();
        fs::write(fixture.driver().join(name), vec![b'x'; 200]).unwrap();
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(AsyncRuntimePlugin)
            .add_plugin(ClosurePlugin)
            .add_closure_system(RuntimePlugin::PRE_UPDATE)
            .add_plugin(ResourcePlugin)
            .add_plugin(AgentDriverPlugin::open(&fixture.0).unwrap().with_limits(
                AgentDriverLoadLimits {
                    max_manifest_bytes: 100,
                    max_script_bytes: 100,
                    ..Default::default()
                },
            ));
        let target = app.world_mut().spawn();
        assert!(load(&mut app, "bounded", target).is_err());
    }
}
#[test]
fn both_files_require_utf8() {
    for name in ["driver.toml", "base.lua"] {
        let fixture = Fixture::new();
        fs::write(fixture.driver().join(name), [0xff]).unwrap();
        let mut app = app(&fixture.0);
        let target = app.world_mut().spawn();
        assert!(load(&mut app, "utf8", target).is_err());
        assert!(app
            .world()
            .get_component::<ResourceSet<AgentDriver>>(target)
            .is_none());
    }
}
#[test]
fn open_does_not_create_root_and_rejects_relative_or_parent_paths() {
    let absent = std::env::temp_dir().join(format!("absent-driver-root-{}", std::process::id()));
    assert!(!absent.exists());
    assert!(AgentDriverPlugin::open(&absent).is_ok());
    assert!(!absent.exists());
    assert!(AgentDriverPlugin::open("relative").is_err());
    assert!(AgentDriverPlugin::open(absent.join("../escape")).is_err());
}
#[cfg(unix)]
#[test]
fn owned_symlink_is_rejected() {
    let fixture = Fixture::new();
    fs::write(fixture.driver().join("other.lua"), "other").unwrap();
    let owned = fixture.driver().join("base.lua");
    assert!(owned.is_absolute() && owned.ends_with("local/coder/latest/base.lua"));
    fs::remove_file(&owned).unwrap();
    std::os::unix::fs::symlink("other.lua", owned).unwrap();
    let mut app = app(&fixture.0);
    let target = app.world_mut().spawn();
    assert!(load(&mut app, "symlink", target).is_err());
}

#[test]
fn missing_driver_directory_reports_resource_attempt_and_cause() {
    let fixture = Fixture::new();
    let mut app = app(&fixture.0);
    let target = app.world_mut().spawn();
    let mut missing = request("missing", target);
    missing.resource_id = ResourceID::parse("driver:local/absent:latest").unwrap();
    app.world().send_event(missing);
    let error = wait(&mut app, &["missing"])
        .pop()
        .unwrap()
        .result
        .unwrap_err();
    assert!(error.message.contains("driver:local/absent:latest"));
    let path = fixture.0.join("local/absent/latest");
    assert!(error.message.contains(path.to_str().unwrap()), "{error}");
    let cause = fs::symlink_metadata(fixture.0.join("local/absent"))
        .unwrap_err()
        .to_string();
    assert!(error.message.contains(&cause), "{error}");
}

#[test]
fn missing_driver_files_report_exact_absolute_path_and_cause() {
    for filename in ["driver.toml", "base.lua"] {
        let fixture = Fixture::new();
        let path = fixture.driver().join(filename);
        assert!(path.is_absolute() && path.parent() == Some(fixture.driver().as_path()));
        fs::remove_file(&path).unwrap();
        let mut app = app(&fixture.0);
        let target = app.world_mut().spawn();
        let error = load(&mut app, "missing-file", target).unwrap_err();
        assert!(
            error.message.contains("driver:local/coder:latest"),
            "{error}"
        );
        assert!(error.message.contains(path.to_str().unwrap()), "{error}");
        let cause = fs::symlink_metadata(&path).unwrap_err().to_string();
        assert!(error.message.contains(&cause), "{error}");
    }
}
