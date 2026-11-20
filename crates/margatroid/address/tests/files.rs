use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use address::{resolve_file, AddressError};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "margatroid-address-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn root(&self) -> &Path {
        &self.0
    }

    fn file(&self, relative: &str) -> PathBuf {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "entry").unwrap();
        path
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
                .starts_with("margatroid-address-")
        {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

#[test]
fn all_four_global_layouts_resolve_to_real_absolute_entries() {
    let fixture = Fixture::new();
    for (kind, directory, entry) in [
        ("driver", "agent-drivers", "base.lua"),
        ("tool", "tools", "tool.toml"),
        ("prompt", "prompts", "prompt.toml"),
        ("sandbox", "sandboxes", "policy.json"),
    ] {
        let expected = fixture.file(&format!(
            ".margatroid/{directory}/margatroid/sample/dev/{entry}"
        ));
        let root = fixture.root().join(".margatroid").join(directory);
        let resolved = resolve_file(&root, &format!("{kind}:margatroid/sample:dev")).unwrap();
        assert!(resolved.is_absolute());
        assert_eq!(resolved, expected.canonicalize().unwrap());
        assert_eq!(
            resolve_file(&root, &format!("{kind}:margatroid/sample:dev")).unwrap(),
            resolved
        );
    }
    let version = fixture.file(".margatroid/tools/margatroid/sample/v2/tool.toml");
    assert_eq!(
        resolve_file(
            &fixture.root().join(".margatroid/tools"),
            "tool:margatroid/sample:v2"
        )
        .unwrap(),
        version.canonicalize().unwrap()
    );
}

#[test]
fn missing_files_non_files_and_invalid_input_are_distinct() {
    let fixture = Fixture::new();
    assert!(matches!(
        resolve_file(fixture.root(), "tool:margatroid/sample:dev"),
        Err(AddressError::FileMissing(_))
    ));
    assert!(matches!(
        resolve_file(
            &fixture.root().join("missing"),
            "tool:margatroid/sample:dev"
        ),
        Err(AddressError::FileMissing(_))
    ));
    fs::create_dir_all(fixture.root().join("margatroid/sample/dev/tool.toml")).unwrap();
    assert!(matches!(
        resolve_file(fixture.root(), "tool:margatroid/sample:dev"),
        Err(AddressError::NotFile(_))
    ));
    assert!(matches!(
        resolve_file(fixture.root(), "agent:margatroid/sample:dev"),
        Err(AddressError::InvalidPart)
    ));
    assert!(matches!(
        resolve_file(fixture.root(), "bad"),
        Err(AddressError::InvalidFormat)
    ));
    for address in [
        "tool:../sample:dev",
        "tool:margatroid/..:dev",
        "tool:margatroid/sample:..",
        "tool:margatroid/sample/../../outside",
        "tool:margatroid/sample:a\\b",
    ] {
        assert!(resolve_file(fixture.root(), address).is_err(), "{address}");
    }
}

#[test]
fn every_resolution_checks_current_filesystem_state() {
    let fixture = Fixture::new();
    let path = fixture.file("margatroid/sample/dev/tool.toml");
    assert!(resolve_file(fixture.root(), "tool:margatroid/sample:dev").is_ok());
    assert_eq!(path, fixture.root().join("margatroid/sample/dev/tool.toml"));
    fs::remove_file(&path).unwrap();
    assert!(matches!(
        resolve_file(fixture.root(), "tool:margatroid/sample:dev"),
        Err(AddressError::FileMissing(_))
    ));
}

#[cfg(unix)]
#[test]
fn symlinks_inside_root_and_symlinked_roots_resolve_canonically() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    let target = fixture.file("real/margatroid/sample/dev/tool.toml");
    let root = fixture.root().join("real");
    let alias = fixture.root().join("alias");
    symlink(&root, &alias).unwrap();
    assert_eq!(
        resolve_file(&alias, "tool:margatroid/sample:dev").unwrap(),
        target.canonicalize().unwrap()
    );
    symlink(root.join("margatroid"), root.join("linked")).unwrap();
    assert_eq!(
        resolve_file(&root, "tool:linked/sample:dev").unwrap(),
        target.canonicalize().unwrap()
    );
    let actual = fixture.file("real/margatroid/sample/dev/actual.toml");
    assert_eq!(target, root.join("margatroid/sample/dev/tool.toml"));
    fs::remove_file(&target).unwrap();
    symlink(&actual, &target).unwrap();
    assert_eq!(
        resolve_file(&root, "tool:margatroid/sample:dev").unwrap(),
        actual.canonicalize().unwrap()
    );
}

#[cfg(unix)]
#[test]
fn directory_and_entry_symlinks_cannot_escape_root() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    let root = fixture.root().join("tools");
    let outside = fixture.file("tools-other/sample/dev/tool.toml");
    fs::create_dir(&root).unwrap();
    symlink(fixture.root().join("tools-other"), root.join("margatroid")).unwrap();
    assert!(matches!(
        resolve_file(&root, "tool:margatroid/sample:dev"),
        Err(AddressError::PathOutsideRoot { .. })
    ));
    assert!(matches!(
        resolve_file(&root, "tool:margatroid/missing:dev"),
        Err(AddressError::PathOutsideRoot { .. })
    ));
    let entry = fixture.file("tools/safe/sample/dev/tool.toml");
    assert_eq!(entry, root.join("safe/sample/dev/tool.toml"));
    fs::remove_file(&entry).unwrap();
    symlink(&outside, &entry).unwrap();
    assert!(matches!(
        resolve_file(&root, "tool:safe/sample:dev"),
        Err(AddressError::PathOutsideRoot { .. })
    ));
}

#[cfg(unix)]
#[test]
fn non_regular_files_are_rejected() {
    use std::os::unix::net::UnixListener;

    let fixture = Fixture::new();
    let directory = fixture.root().join("margatroid/sample/dev");
    fs::create_dir_all(&directory).unwrap();
    let _socket = UnixListener::bind(directory.join("tool.toml")).unwrap();
    assert!(matches!(
        resolve_file(fixture.root(), "tool:margatroid/sample:dev"),
        Err(AddressError::NotFile(_))
    ));
}
