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
        ("driver", "agent-drivers", "driver.toml"),
        ("tool", "tools", "tool.toml"),
        ("prompt", "prompts", "prompt.toml"),
        ("sandbox", "sandboxes", "policy.json"),
    ] {
        let expected = fixture.file(&format!(
            ".margatroid/{directory}/local/sample/latest/{entry}"
        ));
        let root = fixture.root().join(".margatroid").join(directory);
        let resolved = resolve_file(&root, &format!("{kind}:local/sample")).unwrap();
        assert!(resolved.is_absolute());
        assert_eq!(resolved, expected.canonicalize().unwrap());
        assert_eq!(
            resolve_file(&root, &format!("{kind}:local/sample:latest")).unwrap(),
            resolved
        );
    }
    let version = fixture.file(".margatroid/tools/local/sample/v2/tool.toml");
    assert_eq!(
        resolve_file(
            &fixture.root().join(".margatroid/tools"),
            "tool:local/sample:v2"
        )
        .unwrap(),
        version.canonicalize().unwrap()
    );
}

#[test]
fn missing_files_non_files_and_invalid_input_are_distinct() {
    let fixture = Fixture::new();
    assert!(matches!(
        resolve_file(fixture.root(), "tool:local/sample"),
        Err(AddressError::FileMissing(_))
    ));
    assert!(matches!(
        resolve_file(&fixture.root().join("missing"), "tool:local/sample"),
        Err(AddressError::FileMissing(_))
    ));
    fs::create_dir_all(fixture.root().join("local/sample/latest/tool.toml")).unwrap();
    assert!(matches!(
        resolve_file(fixture.root(), "tool:local/sample"),
        Err(AddressError::NotFile(_))
    ));
    assert!(matches!(
        resolve_file(fixture.root(), "agent:local/sample"),
        Err(AddressError::InvalidPart)
    ));
    assert!(matches!(
        resolve_file(fixture.root(), "bad"),
        Err(AddressError::InvalidFormat)
    ));
    for address in [
        "tool:../sample",
        "tool:local/..",
        "tool:local/sample:..",
        "tool:local/sample/../../outside",
        "tool:local/sample:a\\b",
    ] {
        assert!(resolve_file(fixture.root(), address).is_err(), "{address}");
    }
}

#[test]
fn every_resolution_checks_current_filesystem_state() {
    let fixture = Fixture::new();
    let path = fixture.file("local/sample/latest/tool.toml");
    assert!(resolve_file(fixture.root(), "tool:local/sample").is_ok());
    assert_eq!(path, fixture.root().join("local/sample/latest/tool.toml"));
    fs::remove_file(&path).unwrap();
    assert!(matches!(
        resolve_file(fixture.root(), "tool:local/sample"),
        Err(AddressError::FileMissing(_))
    ));
}

#[cfg(unix)]
#[test]
fn symlinks_inside_root_and_symlinked_roots_resolve_canonically() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    let target = fixture.file("real/local/sample/latest/tool.toml");
    let root = fixture.root().join("real");
    let alias = fixture.root().join("alias");
    symlink(&root, &alias).unwrap();
    assert_eq!(
        resolve_file(&alias, "tool:local/sample").unwrap(),
        target.canonicalize().unwrap()
    );
    symlink(root.join("local"), root.join("linked")).unwrap();
    assert_eq!(
        resolve_file(&root, "tool:linked/sample").unwrap(),
        target.canonicalize().unwrap()
    );
    let actual = fixture.file("real/local/sample/latest/actual.toml");
    assert_eq!(target, root.join("local/sample/latest/tool.toml"));
    fs::remove_file(&target).unwrap();
    symlink(&actual, &target).unwrap();
    assert_eq!(
        resolve_file(&root, "tool:local/sample").unwrap(),
        actual.canonicalize().unwrap()
    );
}

#[cfg(unix)]
#[test]
fn directory_and_entry_symlinks_cannot_escape_root() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    let root = fixture.root().join("tools");
    let outside = fixture.file("tools-other/sample/latest/tool.toml");
    fs::create_dir(&root).unwrap();
    symlink(fixture.root().join("tools-other"), root.join("local")).unwrap();
    assert!(matches!(
        resolve_file(&root, "tool:local/sample"),
        Err(AddressError::PathOutsideRoot { .. })
    ));
    assert!(matches!(
        resolve_file(&root, "tool:local/missing"),
        Err(AddressError::PathOutsideRoot { .. })
    ));
    let entry = fixture.file("tools/safe/sample/latest/tool.toml");
    assert_eq!(entry, root.join("safe/sample/latest/tool.toml"));
    fs::remove_file(&entry).unwrap();
    symlink(&outside, &entry).unwrap();
    assert!(matches!(
        resolve_file(&root, "tool:safe/sample"),
        Err(AddressError::PathOutsideRoot { .. })
    ));
}

#[cfg(unix)]
#[test]
fn non_regular_files_are_rejected() {
    use std::os::unix::net::UnixListener;

    let fixture = Fixture::new();
    let directory = fixture.root().join("local/sample/latest");
    fs::create_dir_all(&directory).unwrap();
    let _socket = UnixListener::bind(directory.join("tool.toml")).unwrap();
    assert!(matches!(
        resolve_file(fixture.root(), "tool:local/sample"),
        Err(AddressError::NotFile(_))
    ));
}
