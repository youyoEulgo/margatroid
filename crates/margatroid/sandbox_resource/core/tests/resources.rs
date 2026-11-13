use sandbox_resource_core::{
    read_sandbox, ImportSandbox, SandboxReadLimits, SandboxResourceErrorKind as Kind, Sandboxes,
    SelectedSandbox, WorldSandboxResourceExt,
};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use core_plugin::World;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

static NEXT_ID: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "margatroid-sandbox-core-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn write(&self, name: &str, content: impl AsRef<[u8]>) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, content).unwrap();
        path
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
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
                .starts_with("margatroid-sandbox-core-")
        {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

#[test]
fn path_collections_replace_only_the_named_alias_and_borrow_without_io() {
    let mut entries = Sandboxes::new(crate::test_probe());
    let alpha = PathBuf::from("/missing/alpha");
    let beta = PathBuf::from("/missing/beta");
    assert_eq!(entries.insert("z".into(), alpha.clone()), None);
    entries.insert("a".into(), beta.clone());
    assert_eq!(entries.insert("z".into(), beta.clone()), Some(alpha));
    assert_eq!(
        entries
            .iter()
            .map(|(alias, _)| alias.as_str())
            .collect::<Vec<_>>(),
        ["a", "z"]
    );
    assert_eq!(entries.remove("a"), Some(beta.clone()));
    let mut world = World::new();
    let owner = world.spawn();
    world.insert_component(owner, entries);
    world.insert_component(owner, SelectedSandbox::new(None, test_probe()));
    assert_eq!(world.sandbox_path(owner, "z").unwrap(), beta.as_path());
    assert_eq!(
        world.sandbox_path(owner, "missing").unwrap_err().kind,
        Kind::AliasMissing
    );
    assert_eq!(
        world.sandbox_path(owner, "").unwrap_err().kind,
        Kind::InvalidAlias
    );
    world.despawn(owner);
    let replacement = world.spawn();
    assert_eq!(owner.index(), replacement.index());
    assert_ne!(owner.generation(), replacement.generation());
    assert_eq!(
        world.sandbox_path(owner, "z").unwrap_err().kind,
        Kind::OwnerMissing
    );
    assert_eq!(
        world.sandbox_path(replacement, "z").unwrap_err().kind,
        Kind::AliasMissing
    );
}

#[test]
fn cloned_import_requests_share_a_single_reply() {
    let mut world = World::new();
    let (sender, mut receiver) = oneshot::channel();
    let request = ImportSandbox {
        owner: world.spawn(),
        alias: "sample".into(),
        path: PathBuf::from("/sample"),
        cancel: CancellationToken::new(),
        reply: Arc::new(Mutex::new(Some(sender))),
    };
    let copy = request.clone();
    copy.reply
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .send(Ok(()))
        .unwrap();
    assert!(request.reply.lock().unwrap().is_none());
    assert!(receiver.try_recv().unwrap().is_ok());
    request.cancel.cancel();
    assert!(copy.cancel.is_cancelled());
}

#[test]
fn reads_fresh_json_objects_without_enforcing_backend_schema() {
    let fixture = Fixture::new();
    let path = fixture.write("policy.json", r#"{"custom_backend":{"enabled":true}}"#);
    let first = read_sandbox(&path, &SandboxReadLimits::default()).unwrap();
    assert_eq!(first.origin, path.canonicalize().unwrap());
    fixture.write("policy.json", "{}");
    let second = read_sandbox(&path, &SandboxReadLimits::default()).unwrap();
    assert!(second.policy.as_object().unwrap().is_empty());
    assert!(first.policy.get("custom_backend").is_some());
    assert_eq!(path, fixture.path("policy.json"));
    fs::remove_file(&path).unwrap();
    assert_eq!(
        read_sandbox(&path, &SandboxReadLimits::default())
            .unwrap_err()
            .kind,
        Kind::FileMissing
    );
}

#[test]
fn validates_nonempty_object_utf8_and_exact_byte_budget() {
    let fixture = Fixture::new();
    let path = fixture.write("policy.json", "{}");
    assert!(read_sandbox(&path, &SandboxReadLimits { policy_bytes: 2 }).is_ok());
    assert_eq!(
        read_sandbox(&path, &SandboxReadLimits { policy_bytes: 1 })
            .unwrap_err()
            .kind,
        Kind::TooLarge
    );
    for source in ["", " ", "[]", "null", "42", "{invalid}"] {
        fixture.write("policy.json", source);
        assert_eq!(
            read_sandbox(&path, &SandboxReadLimits::default())
                .unwrap_err()
                .kind,
            Kind::InvalidDefinition
        );
    }
    fixture.write("policy.json", [0xff]);
    assert_eq!(
        read_sandbox(&path, &SandboxReadLimits::default())
            .unwrap_err()
            .kind,
        Kind::InvalidUtf8
    );
    assert_eq!(
        read_sandbox(&fixture.0, &SandboxReadLimits::default())
            .unwrap_err()
            .kind,
        Kind::InvalidPath
    );
}

#[test]
fn selecting_checks_registered_alias_but_does_not_open_policy() {
    let mut world = World::new();
    let owner = world.spawn();
    let mut entries = Sandboxes::new(crate::test_probe());
    entries.insert("safe".into(), PathBuf::from("/missing/policy.json"));
    world.insert_component(owner, entries);
    world.insert_component(owner, SelectedSandbox::new(None, test_probe()));
    world.sandbox_select(owner, Some("safe".into())).unwrap();
    assert_eq!(
        world
            .get_component::<SelectedSandbox>(owner)
            .unwrap()
            .alias
            .as_deref(),
        Some("safe")
    );
    assert_eq!(
        world
            .sandbox_select(owner, Some("missing".into()))
            .unwrap_err()
            .kind,
        Kind::AliasMissing
    );
    assert_eq!(
        world
            .get_component::<SelectedSandbox>(owner)
            .unwrap()
            .alias
            .as_deref(),
        Some("safe")
    );
    world.sandbox_select(owner, None).unwrap();
    assert_eq!(
        world.get_component::<SelectedSandbox>(owner).unwrap().alias,
        None
    );
    world.despawn(owner);
    let replacement = world.spawn();
    assert_eq!(owner.index(), replacement.index());
    assert_eq!(
        world.sandbox_select(owner, None).unwrap_err().kind,
        Kind::OwnerMissing
    );
    assert!(world
        .get_component::<SelectedSandbox>(replacement)
        .is_none());
}

fn test_probe<T>() -> client_core::UpdateProbe<T> {
    client_core::UpdateProbe::new(|_| {})
}
