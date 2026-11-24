use address::{query_resources, ResourceQuery, ResourceRoots};
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "margatroid-resource-query-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn entry(&self, kind: &str, scope: &str, name: &str, tag: &str) -> PathBuf {
        let roots = ResourceRoots::new(&self.0);
        let entry = match kind {
            "driver" => "base.lua",
            "tool" => "tool.toml",
            "prompt" => "prompt.toml",
            "sandbox" => "policy.json",
            _ => unreachable!(),
        };
        let path = roots
            .root(kind)
            .unwrap()
            .join(scope)
            .join(name)
            .join(tag)
            .join(entry);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "opaque resource contents").unwrap();
        path
    }
    fn roots(&self) -> ResourceRoots {
        ResourceRoots::new(&self.0)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
        assert!(self
            .0
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("margatroid-resource-query-"));
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn addresses(roots: &ResourceRoots, q: ResourceQuery) -> Vec<String> {
    query_resources(roots, &q)
        .unwrap()
        .into_iter()
        .map(|r| r.address.to_string())
        .collect()
}
#[test]
fn filters_are_optional_exact_and_combined_and_tags_are_preserved() {
    let f = Fixture::new();
    f.entry("tool", "a", "coder", "dev");
    f.entry("tool", "a", "coder", "v2");
    f.entry("tool", "b", "coder", "dev");
    f.entry("tool", "a", "coders", "dev");
    f.entry("driver", "a", "coder", "dev");
    f.entry("prompt", "a", "context", "dev");
    f.entry("sandbox", "a", "safe", "dev");
    assert_eq!(addresses(&f.roots(), ResourceQuery::default()).len(), 7);
    assert_eq!(
        addresses(
            &f.roots(),
            ResourceQuery {
                resource_type: Some("tool".into()),
                scope: Some("a".into()),
                name: Some("coder".into())
            }
        ),
        ["tool:a/coder:dev", "tool:a/coder:v2"]
    );
    assert_eq!(
        addresses(
            &f.roots(),
            ResourceQuery {
                resource_type: None,
                scope: None,
                name: Some("coder".into())
            }
        )
        .len(),
        4
    );
    assert!(addresses(
        &f.roots(),
        ResourceQuery {
            resource_type: None,
            scope: Some("A".into()),
            name: None
        }
    )
    .is_empty());
}
#[test]
fn actual_paths_include_a_separate_driver_tool_entry_without_changing_resource_rules() {
    let f = Fixture::new();
    let driver = f.entry("driver", "a", "coder", "dev");
    f.entry("tool", "a", "read", "dev");
    f.entry("prompt", "a", "p", "dev");
    let manifest = driver.with_file_name("tool.toml");
    std::fs::write(&manifest, "schema_version=1").unwrap();
    let values = query_resources(&f.roots(), &ResourceQuery::default()).unwrap();
    let record = values.iter().find(|r| r.address.kind == "driver").unwrap();
    assert_eq!(record.path, driver.canonicalize().unwrap());
    assert_eq!(record.tool_path, Some(manifest.canonicalize().unwrap()));
    assert!(values
        .iter()
        .find(|r| r.address.kind == "prompt")
        .unwrap()
        .tool_path
        .is_none());
    let tool = values.iter().find(|r| r.address.kind == "tool").unwrap();
    assert_eq!(tool.tool_path.as_ref(), Some(&tool.path));
    std::fs::remove_file(&manifest).unwrap();
    assert!(query_resources(
        &f.roots(),
        &ResourceQuery {
            resource_type: Some("driver".into()),
            ..Default::default()
        }
    )
    .unwrap()[0]
        .tool_path
        .is_none());
}
#[test]
fn missing_roots_and_incomplete_entries_are_empty_without_creating_files() {
    let f = Fixture::new();
    assert!(query_resources(&f.roots(), &ResourceQuery::default())
        .unwrap()
        .is_empty());
    assert!(!f.0.join("tools").exists());
    std::fs::create_dir_all(f.0.join("tools/a/partial/dev")).unwrap();
    std::fs::write(f.0.join("tools/not-a-directory"), "data").unwrap();
    assert!(query_resources(&f.roots(), &ResourceQuery::default())
        .unwrap()
        .is_empty());
}
#[test]
fn subsequent_queries_read_current_files_without_caching_or_modifying_contents() {
    let f = Fixture::new();
    let first = f.entry("tool", "a", "first", "dev");
    let before = std::fs::read(&first).unwrap();
    assert_eq!(
        addresses(&f.roots(), ResourceQuery::default()),
        ["tool:a/first:dev"]
    );
    assert_eq!(std::fs::read(&first).unwrap(), before);
    let second = f.entry("tool", "a", "second", "dev");
    assert_eq!(addresses(&f.roots(), ResourceQuery::default()).len(), 2);
    std::fs::remove_file(first).unwrap();
    assert_eq!(
        addresses(&f.roots(), ResourceQuery::default()),
        ["tool:a/second:dev"]
    );
    assert!(Path::new(&second).is_file());
}
#[test]
fn invalid_filter_parts_and_unknown_types_fail_before_filesystem_access() {
    let f = Fixture::new();
    for (kind, scope, name) in [
        (Some("unknown"), None, None),
        (Some(""), None, None),
        (None, Some("../escape"), None),
        (None, Some(""), None),
        (None, None, Some("a/b")),
        (None, None, Some("..")),
        (None, None, Some("a:dev")),
    ] {
        assert!(query_resources(
            &f.roots(),
            &ResourceQuery {
                resource_type: kind.map(Into::into),
                scope: scope.map(Into::into),
                name: name.map(Into::into)
            }
        )
        .is_err());
    }
}
#[cfg(unix)]
#[test]
fn directory_and_entry_symlinks_cannot_expose_resources_outside_selected_root() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let outside = Fixture::new();
    let resource = outside.entry("tool", "other", "outside", "dev");
    let valid = f.entry("tool", "a", "valid", "dev");
    let roots = f.roots();
    symlink(
        resource.ancestors().nth(3).unwrap(),
        roots.tool.join("escaped"),
    )
    .unwrap();
    let entry = roots.tool.join("a/symlink/dev");
    std::fs::create_dir_all(&entry).unwrap();
    symlink(&resource, entry.join("tool.toml")).unwrap();
    assert_eq!(
        addresses(&roots, ResourceQuery::default()),
        ["tool:a/valid:dev"]
    );
    assert!(valid.exists());
}

#[cfg(unix)]
#[test]
fn internal_symlink_entries_keep_their_real_path_and_sort_by_parts() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let ordinary = f.entry("tool", "a", "a", "dev");
    let prefix = f.entry("tool", "a", "a-", "dev");
    let real = ordinary.with_file_name("actual-manifest.data");
    assert!(ordinary.starts_with(&f.0));
    std::fs::rename(&ordinary, &real).unwrap();
    symlink(&real, &ordinary).unwrap();
    let rows = query_resources(&f.roots(), &ResourceQuery::default()).unwrap();
    assert_eq!(
        rows.iter()
            .map(|r| r.address.to_string())
            .collect::<Vec<_>>(),
        ["tool:a/a:dev", "tool:a/a-:dev"]
    );
    assert_eq!(rows[0].path, real.canonicalize().unwrap());
    assert_eq!(rows[0].tool_path.as_ref(), Some(&rows[0].path));
    assert_eq!(rows[1].path, prefix.canonicalize().unwrap());
}
