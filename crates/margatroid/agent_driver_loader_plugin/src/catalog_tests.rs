use super::*;
use app_runtime_plugin::WorldEventExt;
use async_runtime_plugin::AsyncRuntimePlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin};
use margatroid_types::discovery::*;
use resource_id_plugin::ResourcePlugin;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static COUNTER: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "margatroid-driver-catalog-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn driver(&self, name: &str) -> PathBuf {
        let path = self.0.join("local").join(name).join("latest");
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("driver.toml"), "schema_version = 1\ndependencies = ['prompt:system/test:v1']\n[inference]\nmodel = 'x'\nstop = ['DONE']\n").unwrap();
        fs::write(path.join("base.lua"), "error('must not execute')").unwrap();
        path
    }
    fn plugin(&self) -> AgentDriverPlugin {
        AgentDriverPlugin::open(&self.0).unwrap()
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
                    .starts_with("margatroid-driver-catalog-")
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
fn wait(app: &mut App, id: &str) -> AgentDriverCatalogResult {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        app.tick();
        if let Some(result) = app
            .world()
            .event_reader::<AgentDriverCatalogResult>()
            .into_iter()
            .find(|result| result.id == id)
        {
            return result.clone();
        }
        assert!(Instant::now() < deadline, "catalog result timed out: {id}");
        std::thread::yield_now();
    }
}
fn scan(fixture: &Fixture) -> AgentDriverCatalogSnapshot {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(scan_validated(
            &fixture.0,
            &AgentDriverLoadLimits::default(),
        ))
        .unwrap()
}
#[test]
fn validates_real_manifests_and_lists_deterministically_without_mounting() {
    let fixture = Fixture::new();
    fixture.driver("zeta");
    fixture.driver("alpha");
    let expected = vec![
        ResourceID::parse("driver:local/alpha").unwrap(),
        ResourceID::parse("driver:local/zeta").unwrap(),
    ];
    assert_eq!(fixture.plugin().scan_catalog().unwrap(), expected);
    assert_eq!(fixture.plugin().scan_catalog().unwrap(), expected);
    let mut app = app(&fixture.0);
    let result = wait(&mut app, "driver-startup").result.unwrap();
    assert_eq!(result.drivers, expected);
    assert!(result.errors.is_empty());
    assert_eq!(
        app.world().get_resource::<DriverCatalog>().unwrap().0,
        expected
    );
    assert_eq!(app.world().entity_count(), 0);
}
#[test]
fn invalid_utf8_schema_missing_files_and_bad_dependencies_are_omitted() {
    let fixture = Fixture::new();
    fixture.driver("valid");
    let utf8 = fixture.driver("utf8");
    fs::write(utf8.join("base.lua"), [0xff]).unwrap();
    let schema = fixture.driver("schema");
    fs::write(
        schema.join("driver.toml"),
        "schema_version = 2\n[inference]\nmodel = 'x'\n",
    )
    .unwrap();
    let missing = fixture.driver("missing");
    fs::remove_file(missing.join("base.lua")).unwrap();
    let dependencies = fixture.driver("dependencies");
    fs::write(
        dependencies.join("driver.toml"),
        "schema_version = 1\ndependencies = ['invalid']\n[inference]\nmodel = 'x'\n",
    )
    .unwrap();
    let manifest_utf8 = fixture.driver("manifest-utf8");
    fs::write(manifest_utf8.join("driver.toml"), [0xff]).unwrap();
    let result = scan(&fixture);
    assert_eq!(
        result.drivers,
        vec![ResourceID::parse("driver:local/valid").unwrap()]
    );
    assert_eq!(result.errors.len(), 5);
    assert!(result
        .errors
        .windows(2)
        .all(|pair| pair[0].path < pair[1].path));
    assert!(result.errors.iter().any(|e| e.message.contains("UTF-8")));
    assert!(result
        .errors
        .iter()
        .any(|e| e.message.contains("schema_version")));
}
#[test]
fn loading_limits_apply_to_catalog_validation() {
    let fixture = Fixture::new();
    fixture.driver("valid");
    assert!(fixture
        .plugin()
        .with_limits(AgentDriverLoadLimits {
            max_script_bytes: 1,
            ..Default::default()
        })
        .scan_catalog()
        .unwrap()
        .is_empty());
}
#[test]
fn missing_root_is_error_and_never_created() {
    let fixture = Fixture::new();
    let absent = fixture.0.join("absent");
    assert!(AgentDriverPlugin::open(&absent)
        .unwrap()
        .scan_catalog()
        .is_err());
    let mut app = app(&absent);
    assert!(wait(&mut app, "driver-startup").result.is_err());
    assert!(!absent.exists());
    assert!(app
        .world()
        .get_resource::<DriverCatalog>()
        .unwrap()
        .0
        .is_empty());
}
#[test]
fn query_is_cached_refresh_removes_stale_and_top_failure_preserves_snapshot() {
    let fixture = Fixture::new();
    let path = fixture.driver("valid");
    let mut app = app(&fixture.0);
    let initial = wait(&mut app, "driver-startup").result.unwrap();
    assert_eq!(initial.drivers.len(), 1);
    assert_eq!(path, fixture.0.join("local/valid/latest"));
    fs::remove_dir_all(&path).unwrap();
    app.world().send_event(QueryAgentDriverCatalog {
        id: "cached".into(),
    });
    assert_eq!(wait(&mut app, "cached").result.unwrap(), initial);
    app.world().send_event(RefreshAgentDriverCatalog {
        id: "deleted".into(),
    });
    assert!(wait(&mut app, "deleted").result.unwrap().drivers.is_empty());
    assert!(app
        .world()
        .get_resource::<DriverCatalog>()
        .unwrap()
        .0
        .is_empty());
    fixture.driver("replacement");
    app.world().send_event(RefreshAgentDriverCatalog {
        id: "replacement".into(),
    });
    let good = wait(&mut app, "replacement").result.unwrap();
    assert_eq!(good.drivers.len(), 1);
    assert!(
        fixture.0.is_absolute()
            && fixture
                .0
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("margatroid-driver-catalog-")
    );
    fs::remove_dir_all(&fixture.0).unwrap();
    app.world().send_event(RefreshAgentDriverCatalog {
        id: "failed".into(),
    });
    assert!(wait(&mut app, "failed").result.is_err());
    assert_eq!(
        app.world()
            .get_resource::<AgentDriverDiscoverySnapshot>()
            .unwrap()
            .0,
        good
    );
    assert_eq!(
        app.world().get_resource::<DriverCatalog>().unwrap().0,
        good.drivers
    );
}
#[test]
fn partial_refresh_publishes_only_valid_packages_and_diagnostics() {
    let fixture = Fixture::new();
    fixture.driver("valid");
    let bad = fixture.driver("bad");
    let mut app = app(&fixture.0);
    assert_eq!(
        wait(&mut app, "driver-startup")
            .result
            .unwrap()
            .drivers
            .len(),
        2
    );
    fs::write(bad.join("driver.toml"), "not toml").unwrap();
    app.world().send_event(RefreshAgentDriverCatalog {
        id: "partial".into(),
    });
    let partial = wait(&mut app, "partial").result.unwrap();
    assert_eq!(partial.drivers.len(), 1);
    assert_eq!(partial.errors.len(), 1);
    assert_eq!(
        app.world().get_resource::<DriverCatalog>().unwrap().0,
        partial.drivers
    );
}
#[test]
fn sync_scan_can_be_called_inside_tokio() {
    let fixture = Fixture::new();
    fixture.driver("valid");
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            assert_eq!(fixture.plugin().scan_catalog().unwrap().len(), 1);
        });
}
#[test]
fn directory_entry_limit_bounds_scan() {
    let fixture = Fixture::new();
    for index in 0..=MAX_CATALOG_DIRECTORY_ENTRIES {
        fs::write(fixture.0.join(format!("noise-{index}")), "").unwrap();
    }
    assert!(fixture
        .plugin()
        .scan_catalog()
        .unwrap_err()
        .message
        .contains("exceeds"));
}
#[test]
fn root_and_package_symlinks_are_rejected() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let valid = fixture.driver("valid");
        symlink(&valid, fixture.0.join("local/valid/linked-tag")).unwrap();
        let bad = fixture.driver("bad");
        fs::remove_file(bad.join("base.lua")).unwrap();
        symlink(valid.join("base.lua"), bad.join("base.lua")).unwrap();
        let result = scan(&fixture);
        assert_eq!(result.drivers.len(), 1);
        assert_eq!(result.errors.len(), 1);
        let link = fixture.0.join("root-link");
        symlink(fixture.0.join("local"), &link).unwrap();
        assert!(AgentDriverPlugin::open(&link)
            .unwrap()
            .scan_catalog()
            .is_err());
    }
}
#[test]
fn relative_and_parent_paths_are_rejected() {
    assert!(AgentDriverPlugin::open("relative").is_err());
    assert!(AgentDriverPlugin::open(std::env::temp_dir().join("../escape")).is_err());
}
