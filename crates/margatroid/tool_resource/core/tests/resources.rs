use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tool_resource_core::{
    read_tool, ImportTool, ToolReadLimits, ToolResourceErrorKind as Kind, Tools,
    WorldToolResourceExt,
};

use core_plugin::World;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

static NEXT_ID: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "margatroid-tool-core-{}-{}",
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
                .starts_with("margatroid-tool-core-")
        {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

#[test]
fn path_collections_replace_only_the_named_alias_and_borrow_without_io() {
    let mut entries = Tools::new();
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
    assert_eq!(world.tool_path(owner, "z").unwrap(), beta.as_path());
    assert_eq!(
        world.tool_path(owner, "missing").unwrap_err().kind,
        Kind::AliasMissing
    );
    assert_eq!(
        world.tool_path(owner, "").unwrap_err().kind,
        Kind::InvalidAlias
    );
    world.despawn(owner);
    let replacement = world.spawn();
    assert_eq!(owner.index(), replacement.index());
    assert_ne!(owner.generation(), replacement.generation());
    assert_eq!(
        world.tool_path(owner, "z").unwrap_err().kind,
        Kind::OwnerMissing
    );
    assert_eq!(
        world.tool_path(replacement, "z").unwrap_err().kind,
        Kind::AliasMissing
    );
}

#[test]
fn cloned_import_requests_share_a_single_reply() {
    let mut world = World::new();
    let (sender, mut receiver) = oneshot::channel();
    let request = ImportTool {
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

fn valid_tool(fixture: &Fixture) -> PathBuf {
    fixture.write("input.schema.json", r#"{"type":"object"}"#);
    fixture.write("main.lua", "return { value = 1 }");
    fixture.write("tool.toml", "schema_version = 1\ndescription = 'sample'\n")
}

#[test]
fn reads_current_materials_without_lua_execution_or_identity_fields() {
    let fixture = Fixture::new();
    let path = valid_tool(&fixture);
    let first = read_tool(&path, &ToolReadLimits::default()).unwrap();
    assert_eq!(first.description, "sample");
    assert_eq!(first.origin, path.canonicalize().unwrap());
    fixture.write("main.lua", "invalid Lua is still UTF-8");
    fixture.write("tool.toml", "schema_version = 1\ndescription = 'changed'\n");
    let second = read_tool(&path, &ToolReadLimits::default()).unwrap();
    assert_eq!(second.description, "changed");
    assert_eq!(second.program, "invalid Lua is still UTF-8");
    assert_eq!(first.description, "sample");
    let program = fixture.path("main.lua");
    assert_eq!(program, path.parent().unwrap().join("main.lua"));
    fs::remove_file(program).unwrap();
    let error = read_tool(&path, &ToolReadLimits::default()).unwrap_err();
    assert_eq!(error.kind, Kind::FileMissing);
    assert!(error.path.is_some());
}

#[test]
fn validates_manifest_and_json_schema_without_external_resolution() {
    let fixture = Fixture::new();
    let path = valid_tool(&fixture);
    for source in [
        "schema_version = 2\ndescription = 'x'",
        "schema_version = 1\ndescription = '  '",
        "SECRET-CONTENT [",
    ] {
        fixture.write("tool.toml", source);
        let error = read_tool(&path, &ToolReadLimits::default()).unwrap_err();
        assert_eq!(error.kind, Kind::InvalidDefinition);
        assert!(!error.to_string().contains("SECRET-CONTENT"));
    }
    valid_tool(&fixture);
    for source in [
        r#"[]"#,
        r#"{"type":"invalid"}"#,
        r#"{"$ref":"https://example.invalid/schema.json"}"#,
        r#"{"$defs":{"unused":{"$ref":"file:///secret"}}}"#,
    ] {
        fixture.write("input.schema.json", source);
        assert_eq!(
            read_tool(&path, &ToolReadLimits::default())
                .unwrap_err()
                .kind,
            Kind::InvalidDefinition,
            "{source}"
        );
    }
    fixture.write("input.schema.json", r##"{"$defs":{"text":{"type":"string"}},"type":"object","properties":{"x":{"$ref":"#/$defs/text"}}}"##);
    assert!(read_tool(&path, &ToolReadLimits::default()).is_ok());
}

#[test]
fn each_file_has_its_own_exact_byte_budget_and_utf8_check() {
    let fixture = Fixture::new();
    let path = valid_tool(&fixture);
    let exact = ToolReadLimits {
        manifest_bytes: fs::metadata(&path).unwrap().len() as usize,
        schema_bytes: fs::metadata(fixture.path("input.schema.json"))
            .unwrap()
            .len() as usize,
        program_bytes: fs::metadata(fixture.path("main.lua")).unwrap().len() as usize,
    };
    assert!(read_tool(&path, &exact).is_ok());
    for limits in [
        ToolReadLimits {
            manifest_bytes: exact.manifest_bytes - 1,
            ..exact
        },
        ToolReadLimits {
            schema_bytes: exact.schema_bytes - 1,
            ..exact
        },
        ToolReadLimits {
            program_bytes: exact.program_bytes - 1,
            ..exact
        },
    ] {
        assert_eq!(read_tool(&path, &limits).unwrap_err().kind, Kind::TooLarge);
    }
    for name in ["tool.toml", "input.schema.json", "main.lua"] {
        valid_tool(&fixture);
        fixture.write(name, [0xff]);
        assert_eq!(
            read_tool(&path, &ToolReadLimits::default())
                .unwrap_err()
                .kind,
            Kind::InvalidUtf8
        );
    }
}

#[cfg(unix)]
#[test]
fn required_files_cannot_follow_symlinks_outside_entry_directory() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let outside = Fixture::new();
    let path = valid_tool(&fixture);
    for name in ["input.schema.json", "main.lua"] {
        let target = outside.write(name, "{}");
        let candidate = fixture.path(name);
        assert_eq!(candidate.parent(), path.parent());
        fs::remove_file(&candidate).unwrap();
        symlink(target, &candidate).unwrap();
        assert_eq!(
            read_tool(&path, &ToolReadLimits::default())
                .unwrap_err()
                .kind,
            Kind::InvalidPath
        );
        fs::remove_file(&candidate).unwrap();
        valid_tool(&fixture);
    }
}
