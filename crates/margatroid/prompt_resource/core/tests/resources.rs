use prompt_resource_core::{
    read_prompt, ImportPrompt, PromptReadLimits, PromptResourceErrorKind as Kind, PromptRole,
    Prompts, WorldPromptResourceExt,
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
            "margatroid-prompt-core-{}-{}",
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
                .starts_with("margatroid-prompt-core-")
        {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

#[test]
fn path_collections_replace_only_the_named_alias_and_borrow_without_io() {
    let mut entries = Prompts::new();
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
    assert_eq!(world.prompt_path(owner, "z").unwrap(), beta.as_path());
    assert_eq!(
        world.prompt_path(owner, "missing").unwrap_err().kind,
        Kind::AliasMissing
    );
    assert_eq!(
        world.prompt_path(owner, "").unwrap_err().kind,
        Kind::InvalidAlias
    );
    world.despawn(owner);
    let replacement = world.spawn();
    assert_eq!(owner.index(), replacement.index());
    assert_ne!(owner.generation(), replacement.generation());
    assert_eq!(
        world.prompt_path(owner, "z").unwrap_err().kind,
        Kind::OwnerMissing
    );
    assert_eq!(
        world.prompt_path(replacement, "z").unwrap_err().kind,
        Kind::AliasMissing
    );
}

#[test]
fn cloned_import_requests_share_a_single_reply() {
    let mut world = World::new();
    let (sender, mut receiver) = oneshot::channel();
    let request = ImportPrompt {
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
fn explicit_roles_and_content_are_read_fresh_for_each_use() {
    let fixture = Fixture::new();
    let path = fixture.write("prompt.toml", "role = 'system'");
    fixture.write("content.md", "first");
    let first = read_prompt(&path, &PromptReadLimits::default()).unwrap();
    for (name, role) in [
        ("system", PromptRole::System),
        ("user", PromptRole::User),
        ("assistant", PromptRole::Assistant),
    ] {
        fixture.write("prompt.toml", format!("role = '{name}'"));
        fixture.write("content.md", name);
        let input = read_prompt(&path, &PromptReadLimits::default()).unwrap();
        assert_eq!(input.role, role);
        assert_eq!(input.content, name);
    }
    assert_eq!(first.role, PromptRole::System);
    assert_eq!(first.content, "first");
    let content = fixture.path("content.md");
    assert_eq!(content.parent(), path.parent());
    fs::remove_file(content).unwrap();
    assert_eq!(
        read_prompt(&path, &PromptReadLimits::default())
            .unwrap_err()
            .kind,
        Kind::FileMissing
    );
}

#[test]
fn invalid_roles_empty_content_utf8_and_budgets_are_rejected() {
    let fixture = Fixture::new();
    let path = fixture.write("prompt.toml", "role = 'system'");
    fixture.write("content.md", "text");
    let exact = PromptReadLimits {
        manifest_bytes: fs::metadata(&path).unwrap().len() as usize,
        content_bytes: 4,
    };
    assert!(read_prompt(&path, &exact).is_ok());
    for limits in [
        PromptReadLimits {
            manifest_bytes: exact.manifest_bytes - 1,
            ..exact
        },
        PromptReadLimits {
            content_bytes: 3,
            ..exact
        },
    ] {
        assert_eq!(
            read_prompt(&path, &limits).unwrap_err().kind,
            Kind::TooLarge
        );
    }
    for manifest in [
        "",
        "role = 'tool'",
        "role = 'SYSTEM'",
        "role = 1",
        "SECRET-CONTENT [",
    ] {
        fixture.write("prompt.toml", manifest);
        let error = read_prompt(&path, &PromptReadLimits::default()).unwrap_err();
        assert_eq!(error.kind, Kind::InvalidDefinition);
        assert!(!error.to_string().contains("SECRET-CONTENT"));
    }
    fixture.write("prompt.toml", "role = 'user'");
    for content in ["", " \n\t"] {
        fixture.write("content.md", content);
        assert_eq!(
            read_prompt(&path, &PromptReadLimits::default())
                .unwrap_err()
                .kind,
            Kind::InvalidDefinition
        );
    }
    fixture.write("content.md", [0xff]);
    assert_eq!(
        read_prompt(&path, &PromptReadLimits::default())
            .unwrap_err()
            .kind,
        Kind::InvalidUtf8
    );
    fixture.write("content.md", "text");
    fixture.write("prompt.toml", [0xff]);
    assert_eq!(
        read_prompt(&path, &PromptReadLimits::default())
            .unwrap_err()
            .kind,
        Kind::InvalidUtf8
    );
}

#[cfg(unix)]
#[test]
fn content_symlinks_outside_the_entry_directory_are_rejected() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let outside = Fixture::new();
    let path = fixture.write("prompt.toml", "role = 'user'");
    symlink(
        outside.write("content.md", "outside"),
        fixture.path("content.md"),
    )
    .unwrap();
    assert_eq!(
        read_prompt(&path, &PromptReadLimits::default())
            .unwrap_err()
            .kind,
        Kind::InvalidPath
    );
}
