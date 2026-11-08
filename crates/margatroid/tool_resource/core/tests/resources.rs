use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tool_resource_core::{
    read_tool, ImportTool, ToolEntry, ToolResourceErrorKind as Kind, Tools, WorldToolResourceExt,
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

fn entry(path: &str, owner: Option<&str>) -> ToolEntry {
    ToolEntry {
        path: PathBuf::from(path),
        description: "sample".into(),
        input_schema: serde_json::json!({"type":"object"}),
        owner_task_id: owner.map(str::to_owned),
        binding: Some(serde_json::json!({"remote_name":"sample"})),
    }
}

#[test]
fn records_reject_conflicts_and_borrow_without_io() {
    let mut entries = Tools::new();
    let alpha = entry("/missing/alpha", None);
    let beta = entry("/missing/beta", Some("task-a"));
    entries.insert("z".into(), alpha.clone()).unwrap();
    entries.insert("a".into(), beta.clone()).unwrap();
    assert_eq!(
        entries.insert("z".into(), beta).unwrap_err().kind,
        Kind::AliasConflict
    );
    assert_eq!(entries.get("z"), Some(&alpha));
    assert_eq!(
        entries
            .iter()
            .map(|(alias, _)| alias.as_str())
            .collect::<Vec<_>>(),
        ["a", "z"]
    );
    entries.remove_owner("task-a");
    assert!(entries.get("a").is_none());
    let mut world = World::new();
    let owner = world.spawn();
    world.insert_component(owner, entries);
    assert_eq!(world.tool_entry(owner, "z").unwrap(), &alpha);
    assert_eq!(world.tool_path(owner, "z").unwrap(), alpha.path.as_path());
    assert_eq!(
        world.tool_entry(owner, "missing").unwrap_err().kind,
        Kind::AliasMissing
    );
    assert_eq!(
        world.tool_entry(owner, "").unwrap_err().kind,
        Kind::InvalidAlias
    );
    world.despawn(owner);
    let replacement = world.spawn();
    assert_eq!(owner.index(), replacement.index());
    assert_ne!(owner.generation(), replacement.generation());
    assert_eq!(
        world.tool_entry(owner, "z").unwrap_err().kind,
        Kind::OwnerMissing
    );
    assert_eq!(
        world.tool_entry(replacement, "z").unwrap_err().kind,
        Kind::AliasMissing
    );
}

#[test]
fn batches_validate_every_record_before_atomic_publication() {
    let mut entries = Tools::new();
    entries
        .insert("existing".into(), entry("/missing/existing", None))
        .unwrap();
    let original = entries.clone();
    for aliases in [["new", "existing"], ["new", "new"]] {
        assert_eq!(
            entries
                .insert_batch(
                    aliases
                        .into_iter()
                        .map(|alias| (alias.into(), entry("/missing/tool", Some("task-a"))))
                        .collect()
                )
                .unwrap_err()
                .kind,
            Kind::AliasConflict
        );
        assert_eq!(entries, original);
    }
    let mut invalid = Vec::new();
    let mut item = entry("relative", None);
    invalid.push(("valid", item.clone(), Kind::InvalidPath));
    item.path = PathBuf::from("/missing/tool");
    item.description.clear();
    invalid.push(("valid", item.clone(), Kind::InvalidDefinition));
    item.description = "sample".into();
    for schema in [
        serde_json::json!([]),
        serde_json::json!({"type":"invalid"}),
        serde_json::json!({"$ref":"https://example.invalid"}),
    ] {
        item.input_schema = schema;
        invalid.push(("valid", item.clone(), Kind::InvalidDefinition));
    }
    invalid.push(("1invalid", entry("/missing/tool", None), Kind::InvalidAlias));
    for (alias, item, kind) in invalid {
        assert_eq!(
            entries
                .insert_batch(vec![
                    ("new".into(), entry("/missing/new", None)),
                    (alias.into(), item)
                ])
                .unwrap_err()
                .kind,
            kind
        );
        assert_eq!(entries, original);
    }
    entries
        .insert_batch(vec![
            ("dynamic".into(), entry("/missing/tool", Some("task-a"))),
            ("other".into(), entry("/missing/tool", Some("task-b"))),
        ])
        .unwrap();
    entries.remove_owner("task-a");
    assert!(entries.get("dynamic").is_none());
    assert!(entries.get("other").is_some());
    assert!(entries.get("existing").is_some());
    entries.insert_batch(Vec::new()).unwrap();
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
    fixture.write("main.lua", "function configure(arguments, context) return {timeout_ms=30000} end\nfunction execute() margatroid.task:return_result({ value = 1 }) end");
    fixture.write("tool.toml", "schema_version = 1\ndescription = 'sample'\n")
}

#[test]
fn reads_current_materials_without_lua_execution_or_identity_fields() {
    let fixture = Fixture::new();
    let path = valid_tool(&fixture);
    let first = read_tool(&path).unwrap();
    assert_eq!(first.description, "sample");
    assert_eq!(first.origin, path.canonicalize().unwrap());
    fixture.write("main.lua", "invalid Lua is still UTF-8");
    fixture.write("tool.toml", "schema_version = 1\ndescription = 'changed'\n");
    let second = read_tool(&path).unwrap();
    assert_eq!(second.description, "changed");
    assert_eq!(second.program, "invalid Lua is still UTF-8");
    assert_eq!(first.description, "sample");
    let program = fixture.path("main.lua");
    assert_eq!(program, path.parent().unwrap().join("main.lua"));
    fs::remove_file(program).unwrap();
    let error = read_tool(&path).unwrap_err();
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
        let error = read_tool(&path).unwrap_err();
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
            read_tool(&path).unwrap_err().kind,
            Kind::InvalidDefinition,
            "{source}"
        );
    }
    fixture.write("input.schema.json", r##"{"$defs":{"text":{"type":"string"}},"type":"object","properties":{"x":{"$ref":"#/$defs/text"}}}"##);
    assert!(read_tool(&path).is_ok());
}

#[test]
fn reads_complete_materials_without_an_additional_byte_budget() {
    let fixture = Fixture::new();
    let path = valid_tool(&fixture);
    let description = "x".repeat(1024 * 1024 + 1);
    fixture.write(
        "tool.toml",
        format!("schema_version = 1\ndescription = '{description}'\n"),
    );
    let schema_source = fs::read_to_string(fixture.path("input.schema.json")).unwrap();
    let program = fs::read_to_string(fixture.path("main.lua")).unwrap();
    let input = read_tool(&path).unwrap();
    assert_eq!(input.description, description);
    assert_eq!(
        input.input_schema,
        serde_json::from_str::<serde_json::Value>(&schema_source).unwrap()
    );
    assert_eq!(input.program, program);
}

#[test]
fn invalid_utf8_in_each_file_reports_its_source_path() {
    let fixture = Fixture::new();
    let path = valid_tool(&fixture);
    for name in ["tool.toml", "input.schema.json", "main.lua"] {
        valid_tool(&fixture);
        let source = fixture.write(name, [0xff]);
        let error = read_tool(&path).unwrap_err();
        assert_eq!(error.kind, Kind::InvalidUtf8);
        assert_eq!(error.path, Some(source.canonicalize().unwrap()));
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
        assert_eq!(read_tool(&path).unwrap_err().kind, Kind::InvalidPath);
        fs::remove_file(&candidate).unwrap();
        valid_tool(&fixture);
    }
}
