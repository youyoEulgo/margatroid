use compose::{
    compile, compile_str, ComposeErrorKind, FileWorkspaceCatalogStore, MAX_WORKSPACE_YAML_BYTES,
};
use margatroid_types::workspace_catalog::WorkspaceCatalogStore;
use margatroid_types::workspace_components::WorkspaceErrorKind;
use resource_id_plugin::ResourceID;
use std::{fs, path::PathBuf, sync::Arc};

struct Fixture {
    _temp: tempfile::TempDir,
    project: PathBuf,
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let root = temp.path().join("workspaces");
        fs::create_dir(&project).unwrap();
        Self {
            _temp: temp,
            project,
            root,
        }
    }
    fn path(&self, name: &str) -> PathBuf {
        self.root.join(name).join("workspace.yaml")
    }
    fn yaml(&self, name: &str, agents: &str) -> String {
        format!(
            "name: {name}\nproject_root: {}\nagents: {agents}\n",
            self.project.display()
        )
    }
    fn definition(&self, name: &str) -> margatroid_types::WorkspaceDefinition {
        compile_str(
            &self.yaml(name, "{coder: {driver: local/coder, home_root: home}}"),
            self.path(name),
        )
        .unwrap()
    }
    fn two_member_definition(&self, name: &str) -> margatroid_types::WorkspaceDefinition {
        compile_str(
            &self.yaml(
                name,
                "{coder: {driver: local/coder, home_root: home}, \
                 planner: {driver: driver:local/planner:v2, home_root: homes/planner}}",
            ),
            self.path(name),
        )
        .unwrap()
    }
    /// Give a saved workspace real per-member state so renames must not lose it.
    fn populate(&self, definition: &margatroid_types::WorkspaceDefinition) {
        for (index, agent) in definition.agents.iter().enumerate() {
            let database = &agent.history_path;
            fs::create_dir_all(database.parent().unwrap()).unwrap();
            drop(memory_plugin::SqliteAgentDatabase::open(database).unwrap());
            let connection = rusqlite::Connection::open(database).unwrap();
            connection.execute("INSERT INTO block_definitions(id,agent,instance,name,temporary,schema_json) VALUES (42,?1,'','conversation',0,'{}')", [agent.id.to_string()]).unwrap();
            connection
                .execute(
                    "INSERT INTO block_fields VALUES (42,'messages','MESSAGE','[]')",
                    [],
                )
                .unwrap();
            connection.execute("INSERT INTO history_records(sequence,key,source,turn,kind,content,payload,created_at_ms) VALUES (9,'audit',?1,'turn','message','original old identity','{}',1234)", [agent.id.to_string()]).unwrap();
            let journal = database.parent().unwrap().join("journal.log");
            fs::write(journal, format!("journal-{index}")).unwrap();
        }
        let extras = self.root.join(&definition.name).join("notes");
        fs::create_dir_all(&extras).unwrap();
        fs::write(extras.join("keep.txt"), b"workspace detail").unwrap();
    }
    fn state(&self, name: &str, agent: &str) -> PathBuf {
        self.root
            .join(name)
            .join("agents")
            .join(agent)
            .join("state.sqlite")
    }
}

#[test]
fn documented_public_api_compiles_managed_paths_without_creating_data() {
    let f = Fixture::new();
    let definition = f.definition("demo");
    assert_eq!(definition.id.to_string(), "workspace:local/demo:latest");
    assert_eq!(
        definition.agents[0].driver.to_string(),
        "driver:local/coder:latest"
    );
    assert_eq!(
        definition.agents[0].history_path,
        f.root.join("demo/agents/coder/state.sqlite")
    );
    assert_eq!(definition.agents[0].home_root, f.project.join("home"));
    assert!(!f.root.exists());
    assert!(!f.project.join(".margatroid").exists());
}

#[test]
fn accepts_empty_and_sequence_agents_but_rejects_duplicates() {
    let f = Fixture::new();
    assert!(compile_str(&f.yaml("demo", "[]"), f.path("demo"))
        .unwrap()
        .agents
        .is_empty());
    let yaml = f.yaml("demo", "[{name: coder, driver: local/coder}]");
    assert_eq!(compile_str(&yaml, f.path("demo")).unwrap().agents.len(), 1);
    for agents in [
        "[{name: coder, driver: local/coder}, {name: coder, driver: local/coder}]",
        "{coder: {driver: local/coder}, coder: {driver: local/coder}}",
    ] {
        assert_eq!(
            compile_str(&f.yaml("demo", agents), f.path("demo"))
                .unwrap_err()
                .kind(),
            ComposeErrorKind::DuplicateAgent
        );
    }
}

#[test]
fn rejects_legacy_manager_and_database_overrides() {
    let f = Fixture::new();
    let source = format!("{}manager: coder\n", f.yaml("demo", "[]"));
    assert_eq!(
        compile_str(&source, f.path("demo")).unwrap_err().kind(),
        ComposeErrorKind::FileDecode
    );
    for key in ["history_path", "memory_path", "tag", "resources", "image"] {
        let yaml = f.yaml(
            "demo",
            &format!("{{coder: {{driver: local/coder, {key}: forbidden}}}}"),
        );
        assert_eq!(
            compile_str(&yaml, f.path("demo")).unwrap_err().kind(),
            ComposeErrorKind::FileDecode,
            "{key}"
        );
    }
}

#[test]
fn requires_explicit_existing_absolute_canonical_project() {
    let f = Fixture::new();
    for source in [
        "name: demo\nagents: []\n".to_owned(),
        "name: demo\nproject_root: relative\nagents: []\n".to_owned(),
        f.yaml("demo", "[]").replace(
            &f.project.display().to_string(),
            &f.project.join("missing").display().to_string(),
        ),
    ] {
        assert!(compile_str(&source, f.path("demo")).is_err());
    }
    assert!(compile_str(&f.yaml("demo", "[]"), f.path("other")).is_err());
    assert!(compile_str(&f.yaml("demo", "[]"), f.root.join("demo/other.yaml")).is_err());
    assert!(compile_str(&f.yaml("demo", "[]"), "workspaces/demo/workspace.yaml").is_err());
}

#[test]
fn compile_reads_only_managed_regular_bounded_yaml() {
    let f = Fixture::new();
    let path = f.path("demo");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, f.yaml("demo", "[]")).unwrap();
    assert_eq!(compile(&path).unwrap().project_root, f.project);
    fs::write(&path, vec![b' '; MAX_WORKSPACE_YAML_BYTES + 1]).unwrap();
    assert!(compile(&path).is_err());
    assert!(compile_str(&" ".repeat(MAX_WORKSPACE_YAML_BYTES + 1), &path).is_err());
    assert_eq!(
        compile_str("name: [", &path).unwrap_err().kind(),
        ComposeErrorKind::FileDecode
    );
}

#[test]
fn catalog_roundtrip_hashes_source_and_compares_revisions() {
    let f = Fixture::new();
    let store = FileWorkspaceCatalogStore::new(&f.root).unwrap();
    let definition = f.definition("demo");
    let first = store.save(&definition, "").unwrap();
    assert_eq!(first.revision.len(), 64);
    assert_eq!(store.list().unwrap()[0].revision, first.revision);
    assert_eq!(
        store
            .load(&definition.id, &first.revision)
            .unwrap()
            .revision,
        first.revision
    );
    assert_eq!(
        store.save(&definition, "").unwrap_err().kind(),
        WorkspaceErrorKind::WorkspaceBusy
    );
    let same = store.save(&definition, &first.revision).unwrap();
    assert_eq!(first.revision, same.revision);
    let source = fs::read_to_string(&first.path).unwrap();
    fs::write(&first.path, format!("# external edit\n{source}")).unwrap();
    assert_eq!(
        store
            .load(&definition.id, &first.revision)
            .unwrap_err()
            .kind(),
        WorkspaceErrorKind::WorkspaceBusy
    );
    assert!(store.save(&definition, &first.revision).is_err());
    let refreshed = store.list().unwrap().pop().unwrap();
    assert_ne!(refreshed.revision, first.revision);
    assert!(refreshed.definition.is_some());
    assert!(fs::read_to_string(first.path)
        .unwrap()
        .starts_with("# external edit"));
    assert!(!f.project.join(".margatroid").exists());
}

#[test]
fn catalog_preserves_diagnostics_for_invalid_missing_and_deleted_yaml() {
    let f = Fixture::new();
    let store = FileWorkspaceCatalogStore::new(&f.root).unwrap();
    let definition = f.definition("deleted");
    let saved = store.save(&definition, "").unwrap();
    fs::create_dir_all(f.root.join("deleted/agents/coder")).unwrap();
    fs::remove_file(saved.path).unwrap();
    fs::create_dir_all(f.root.join("broken")).unwrap();
    fs::write(f.path("broken"), "name: [").unwrap();
    fs::create_dir_all(f.root.join("mismatch")).unwrap();
    fs::write(f.path("mismatch"), f.yaml("wrong", "[]")).unwrap();
    let entries = store.list().unwrap();
    assert_eq!(
        entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
        ["broken", "deleted", "mismatch"]
    );
    assert!(entries
        .iter()
        .all(|e| e.error.is_some() && e.definition.is_none()));
    assert!(!entries[0].revision.is_empty());
    assert!(entries[1].revision.is_empty());
    assert!(f.root.join("deleted/agents/coder").exists());
}

#[test]
fn save_rejects_supplied_agent_identity_and_database_escape() {
    let f = Fixture::new();
    let store = FileWorkspaceCatalogStore::new(&f.root).unwrap();
    let mut definition = f.definition("demo");
    definition.agents[0].history_path = f.project.join("state.sqlite");
    assert!(store.save(&definition, "").is_err());
    assert!(!f.path("demo").exists());
    definition = f.definition("demo");
    definition.agents[0].id = resource_id_plugin::ResourceID::parse("agent:other/coder").unwrap();
    assert!(store.save(&definition, "").is_err());
}

#[test]
fn separate_store_instances_serialize_competing_saves() {
    let f = Fixture::new();
    let first = Arc::new(FileWorkspaceCatalogStore::new(&f.root).unwrap());
    let second = Arc::new(FileWorkspaceCatalogStore::new(&f.root).unwrap());
    let definition = f.definition("demo");
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let threads: Vec<_> = [first, second]
        .into_iter()
        .map(|store| {
            let definition = definition.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                store.save(&definition, "")
            })
        })
        .collect();
    assert_eq!(
        threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .filter(Result::is_ok)
            .count(),
        1
    );
}

#[cfg(unix)]
#[test]
fn rejects_symlink_roots_files_and_agent_storage() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let store = FileWorkspaceCatalogStore::new(&f.root).unwrap();
    fs::create_dir(f.root.join("demo")).unwrap();
    symlink(&f.project, f.root.join("demo/agents")).unwrap();
    assert!(compile_str(
        &f.yaml("demo", "{coder: {driver: local/coder}}"),
        f.path("demo")
    )
    .is_err());
    let definition = compile_str(&f.yaml("demo", "[]"), f.path("demo")).unwrap();
    symlink(f.project.join("external.yaml"), f.path("demo")).unwrap();
    assert!(store.save(&definition, "").is_err());
    assert!(!f.project.join("external.yaml").exists());
    assert!(store.list().unwrap()[0].error.is_some());
    let other = f._temp.path().join("other");
    fs::create_dir(&other).unwrap();
    symlink(&f.root, other.join("workspaces")).unwrap();
    assert!(FileWorkspaceCatalogStore::new(other.join("workspaces")).is_err());
}

#[cfg(unix)]
#[test]
fn existing_agent_database_symlink_invalidates_catalog_without_touching_target() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let store = FileWorkspaceCatalogStore::new(&f.root).unwrap();
    let definition = f.definition("demo");
    let saved = store.save(&definition, "").unwrap();
    let database = &definition.agents[0].history_path;
    fs::create_dir_all(database.parent().unwrap()).unwrap();
    let external = f.project.join("external.sqlite");
    fs::write(&external, b"existing state").unwrap();
    symlink(&external, database).unwrap();
    let entry = store.load(&definition.id, &saved.revision).unwrap();
    assert!(entry.definition.is_none());
    assert!(entry.error.is_some());
    assert!(store.save(&definition, &saved.revision).is_err());
    assert_eq!(fs::read(external).unwrap(), b"existing state");
    assert_eq!(store.list().unwrap()[0].revision, saved.revision);
}

#[test]
fn rename_moves_home_and_readdresses_members_without_touching_data() {
    let f = Fixture::new();
    let store = FileWorkspaceCatalogStore::new(&f.root).unwrap();
    let definition = f.two_member_definition("old");
    let saved = store.save(&definition, "").unwrap();
    f.populate(&definition);
    let renamed = store
        .rename(&definition.id, &saved.revision, "new", &definition)
        .unwrap();
    assert_eq!(renamed.id.to_string(), "workspace:local/new:latest");
    assert_eq!(renamed.name, "new");
    assert_eq!(renamed.path, f.path("new"));
    assert_eq!(renamed.error, None);
    assert_ne!(renamed.revision, saved.revision);
    let stored = renamed.definition.clone().unwrap();
    assert_eq!(stored.name, "new");
    assert_eq!(stored.project_root, definition.project_root);
    assert_eq!(
        stored
            .agents
            .iter()
            .map(|agent| agent.id.to_string())
            .collect::<Vec<_>>(),
        ["agent:new/coder:latest", "agent:new/planner:latest"]
    );
    for (actual, before) in stored.agents.iter().zip(&definition.agents) {
        assert_eq!(actual.name, before.name);
        assert_eq!(actual.driver, before.driver);
        assert_eq!(actual.home_root, before.home_root);
        assert_eq!(actual.history_path, f.state("new", &actual.name));
        let connection = rusqlite::Connection::open(&actual.history_path).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT id,agent,instance FROM block_definitions", [], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })
                .unwrap(),
            (42, actual.id.to_string(), String::new())
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT sequence,source,content,created_at_ms FROM history_records",
                    [],
                    |r| Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, i64>(3)?
                    ))
                )
                .unwrap(),
            (
                9,
                before.id.to_string(),
                "original old identity".into(),
                1234
            )
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT value_json FROM block_fields WHERE definition_id=42",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            "[]"
        );
        assert!(actual
            .history_path
            .parent()
            .unwrap()
            .join("journal.log")
            .is_file());
    }
    assert_eq!(
        fs::read(f.root.join("new/notes/keep.txt")).unwrap(),
        b"workspace detail"
    );
    assert!(!f.root.join("old").exists());
    let source = fs::read_to_string(f.path("new")).unwrap();
    assert!(source.contains("name: new"));
    assert!(!source.contains("new/old"));
    assert_eq!(
        store
            .load(&definition.id, &saved.revision)
            .unwrap_err()
            .kind(),
        WorkspaceErrorKind::WorkspaceBusy
    );
    assert_eq!(
        store.load(&renamed.id, "").unwrap_err().kind(),
        WorkspaceErrorKind::WorkspaceBusy
    );
    assert_eq!(
        store.load(&renamed.id, &renamed.revision).unwrap().name,
        "new"
    );
    let entries = store.list().unwrap();
    assert_eq!(
        entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
        ["new"]
    );
    assert_eq!(entries[0].revision, renamed.revision);
    let leftovers = fs::read_dir(f.root.join("new"))
        .unwrap()
        .map(|item| item.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with('.'))
        .collect::<Vec<_>>();
    assert!(
        leftovers.is_empty(),
        "staged files left behind: {leftovers:?}"
    );
}

#[test]
fn rename_accepts_equivalent_definition_and_preserves_unsaved_comments() {
    let f = Fixture::new();
    let store = FileWorkspaceCatalogStore::new(&f.root).unwrap();
    let definition = f.definition("old");
    let saved = store.save(&definition, "").unwrap();
    let source = fs::read_to_string(&saved.path).unwrap();
    fs::write(&saved.path, format!("# team note\n{source}")).unwrap();
    assert_eq!(
        store.load(&definition.id, "").unwrap_err().kind(),
        WorkspaceErrorKind::WorkspaceBusy
    );
    let current = store.list().unwrap().pop().unwrap();
    let mut stale = current.definition.clone().unwrap();
    stale.agents[0].driver = ResourceID::parse("driver:local/other").unwrap();
    assert_eq!(
        store
            .rename(&definition.id, &saved.revision, "new", &stale)
            .unwrap_err()
            .kind(),
        WorkspaceErrorKind::WorkspaceBusy
    );
    assert_eq!(
        store
            .rename(&definition.id, &current.revision, "new", &stale)
            .unwrap_err()
            .kind(),
        WorkspaceErrorKind::InvalidDefinition
    );
    assert!(f.path("old").is_file());
    let renamed = store
        .rename(
            &definition.id,
            &current.revision,
            "new",
            &current.definition.clone().unwrap(),
        )
        .unwrap();
    assert_eq!(renamed.name, "new");
    assert!(store.list().unwrap().iter().all(|e| e.name == "new"));
}

#[test]
fn rename_rejects_stale_revision_unchanged_name_and_existing_target() {
    let f = Fixture::new();
    let store = FileWorkspaceCatalogStore::new(&f.root).unwrap();
    let definition = f.definition("alpha");
    let saved = store.save(&definition, "").unwrap();
    let other = f.definition("beta");
    store.save(&other, "").unwrap();
    let alpha_source = fs::read_to_string(f.path("alpha")).unwrap();
    let beta_source = fs::read_to_string(f.path("beta")).unwrap();
    assert_eq!(
        store
            .rename(&definition.id, "", "gamma", &definition)
            .unwrap_err()
            .kind(),
        WorkspaceErrorKind::WorkspaceBusy
    );
    assert_eq!(
        store
            .rename(&definition.id, &saved.revision, "alpha", &definition)
            .unwrap_err()
            .kind(),
        WorkspaceErrorKind::InvalidDefinition
    );
    assert_eq!(
        store
            .rename(&definition.id, &saved.revision, "beta", &definition)
            .unwrap_err()
            .kind(),
        WorkspaceErrorKind::DuplicateWorkspace
    );
    for invalid in ["", ".", "..", ".trash", "a/b", "with space/nested"] {
        assert!(
            store
                .rename(&definition.id, &saved.revision, invalid, &definition)
                .is_err(),
            "{invalid}"
        );
    }
    fs::create_dir(f.root.join("half-made")).unwrap();
    assert_eq!(
        store
            .rename(&definition.id, &saved.revision, "half-made", &definition)
            .unwrap_err()
            .kind(),
        WorkspaceErrorKind::DuplicateWorkspace
    );
    let mut foreign = definition.clone();
    foreign.name = "gamma".into();
    foreign.id = ResourceID::parse("workspace:local/gamma").unwrap();
    assert_eq!(
        store
            .rename(&definition.id, &saved.revision, "gamma", &foreign)
            .unwrap_err()
            .kind(),
        WorkspaceErrorKind::InvalidDefinition
    );
    assert_eq!(fs::read_to_string(f.path("alpha")).unwrap(), alpha_source);
    assert_eq!(fs::read_to_string(f.path("beta")).unwrap(), beta_source);
    assert!(!f.root.join("gamma").exists());
    assert_eq!(
        store
            .list()
            .unwrap()
            .iter()
            .map(|e| e.name.as_str())
            .collect::<Vec<_>>(),
        ["alpha", "beta", "half-made"]
    );
}

#[cfg(unix)]
#[test]
fn rename_rejects_symlinked_homes_and_targets_without_touching_targets() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let store = FileWorkspaceCatalogStore::new(&f.root).unwrap();
    let external = f.project.join("external");
    fs::create_dir(&external).unwrap();
    fs::write(external.join("workspace.yaml"), "name: gamma\n").unwrap();
    let definition = f.definition("alpha");
    let saved = store.save(&definition, "").unwrap();
    symlink(&external, f.root.join("gamma")).unwrap();
    assert_eq!(
        store
            .rename(&definition.id, &saved.revision, "gamma", &definition)
            .unwrap_err()
            .kind(),
        WorkspaceErrorKind::DuplicateWorkspace
    );
    assert_eq!(
        store.load(&definition.id, &saved.revision).unwrap().name,
        "alpha"
    );
    assert_eq!(
        fs::read_to_string(external.join("workspace.yaml")).unwrap(),
        "name: gamma\n"
    );
    symlink(&external, f.project.join("alias")).unwrap();
    let alias = f.project.join("alias");
    assert!(compile_str(
        &f.yaml(
            "alpha",
            &format!(
                "{{coder: {{driver: local/coder, home_root: {}}}}}",
                alias.display()
            )
        ),
        f.path("alias-home")
    )
    .is_err());
    let mut renamed_target = f.definition("alpha");
    renamed_target.agents[0].home_root = alias;
    assert!(store
        .rename(&definition.id, &saved.revision, "delta", &renamed_target)
        .is_err());
    assert_eq!(
        store.load(&definition.id, &saved.revision).unwrap().name,
        "alpha"
    );
    assert!(!f.root.join("delta").exists());
}

#[test]
fn delete_moves_whole_home_into_unlisted_trash_without_data_loss() {
    let f = Fixture::new();
    let store = FileWorkspaceCatalogStore::new(&f.root).unwrap();
    let definition = f.two_member_definition("demo");
    let saved = store.save(&definition, "").unwrap();
    f.populate(&definition);
    let original_databases: Vec<_> = definition
        .agents
        .iter()
        .map(|agent| fs::read(&agent.history_path).unwrap())
        .collect();
    fs::write(
        &saved.path,
        format!("# edited\n{}", fs::read_to_string(&saved.path).unwrap()),
    )
    .unwrap();
    assert_eq!(
        store
            .delete(&definition.id, &saved.revision)
            .unwrap_err()
            .kind(),
        WorkspaceErrorKind::WorkspaceBusy
    );
    let restored = fs::read_to_string(f.path("demo"))
        .unwrap()
        .trim_start_matches("# edited\n")
        .to_owned();
    fs::write(&saved.path, &restored).unwrap();
    let current = store.list().unwrap().pop().unwrap();
    let deletion = store.delete(&definition.id, &current.revision).unwrap();
    assert_eq!(
        store
            .load(&definition.id, &current.revision)
            .unwrap_err()
            .kind(),
        WorkspaceErrorKind::WorkspaceBusy
    );
    assert_eq!(deletion.id, definition.id);
    assert_eq!(deletion.path, f.root.join("demo"));
    assert!(!f.root.join("demo").exists());
    assert!(!deletion.removed_path.starts_with(&f.root.join("demo")));
    assert_eq!(
        deletion.removed_path.parent(),
        Some(store.trash_root().as_path())
    );
    assert!(deletion
        .removed_path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("demo."));
    assert!(store.list().unwrap().is_empty());
    assert_eq!(
        fs::read_to_string(deletion.removed_path.join("workspace.yaml")).unwrap(),
        restored
    );
    for (agent, original_bytes) in definition.agents.iter().zip(original_databases) {
        let database = deletion
            .removed_path
            .join("agents")
            .join(&agent.name)
            .join("state.sqlite");
        assert_eq!(fs::read(&database).unwrap(), original_bytes);
    }
    assert_eq!(
        fs::read(deletion.removed_path.join("notes/keep.txt")).unwrap(),
        b"workspace detail"
    );
}

#[test]
fn delete_rejects_missing_workspaces_and_reserved_names() {
    let f = Fixture::new();
    let store = FileWorkspaceCatalogStore::new(&f.root).unwrap();
    let missing = ResourceID::parse("workspace:local/absent").unwrap();
    assert_eq!(
        store.delete(&missing, "").unwrap_err().kind(),
        WorkspaceErrorKind::WorkspaceBusy
    );
    assert_eq!(
        store.delete(&missing, "deadbeef").unwrap_err().kind(),
        WorkspaceErrorKind::WorkspaceBusy
    );
    assert!(!store.trash_root().exists());
    let trash = ResourceID::parse("workspace:local/.trash").unwrap();
    assert!(store.delete(&trash, "").is_err());
}

#[test]
fn member_rename_migrates_only_member_and_rejects_retained_target() {
    let f = Fixture::new();
    let store = FileWorkspaceCatalogStore::new(&f.root).unwrap();
    let definition = f.two_member_definition("demo");
    let saved = store.save(&definition, "").unwrap();
    f.populate(&definition);
    let untouched = fs::read(&definition.agents[1].history_path).unwrap();
    let live_other = rusqlite::Connection::open(&definition.agents[1].history_path).unwrap();
    fs::create_dir_all(f.root.join("demo/agents/retained")).unwrap();
    assert!(store
        .rename_member(
            &definition.id,
            &saved.revision,
            &definition.agents[0].id,
            "retained"
        )
        .is_err());
    let renamed = store
        .rename_member(
            &definition.id,
            &saved.revision,
            &definition.agents[0].id,
            "renamed",
        )
        .unwrap();
    let current = renamed.definition.unwrap();
    assert_eq!(
        current.agents[0].id.to_string(),
        "agent:demo/renamed:latest"
    );
    assert!(!f.root.join("demo/agents/coder").exists());
    assert_eq!(
        fs::read(&current.agents[1].history_path).unwrap(),
        untouched
    );
    live_other.execute("INSERT INTO history_records(key,source,kind,content,payload) VALUES ('after-rename','still-live','message','kept','{}')", []).unwrap();
    let reopen_other = rusqlite::Connection::open(&current.agents[1].history_path).unwrap();
    assert_eq!(
        reopen_other
            .query_row(
                "SELECT content FROM history_records WHERE key='after-rename'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "kept"
    );
    let db = rusqlite::Connection::open(&current.agents[0].history_path).unwrap();
    assert_eq!(
        db.query_row("SELECT agent FROM block_definitions WHERE id=42", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "agent:demo/renamed:latest"
    );
    assert_eq!(
        db.query_row(
            "SELECT source FROM history_records WHERE sequence=9",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "agent:demo/coder:latest"
    );
}

#[test]
fn workspace_rename_migrates_retained_member_databases() {
    let f = Fixture::new();
    let store = FileWorkspaceCatalogStore::new(&f.root).unwrap();
    let definition = f.two_member_definition("old");
    let saved = store.save(&definition, "").unwrap();
    f.populate(&definition);
    let mut reduced = definition.clone();
    reduced.agents.pop();
    let current = store.save(&reduced, &saved.revision).unwrap();
    store
        .rename_workspace(&reduced.id, &current.revision, "new", &reduced)
        .unwrap();
    let db = rusqlite::Connection::open(f.state("new", "planner")).unwrap();
    assert_eq!(
        db.query_row("SELECT agent FROM block_definitions WHERE id=42", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "agent:new/planner:latest"
    );
}

#[test]
fn unknown_identity_or_schema_leaves_original_home_unchanged() {
    for unknown_schema in [false, true] {
        let f = Fixture::new();
        let store = FileWorkspaceCatalogStore::new(&f.root).unwrap();
        let definition = f.definition("old");
        let saved = store.save(&definition, "").unwrap();
        f.populate(&definition);
        let db = rusqlite::Connection::open(&definition.agents[0].history_path).unwrap();
        if unknown_schema {
            db.execute("CREATE TABLE unknown_identity (agent TEXT)", [])
                .unwrap();
        } else {
            db.execute(
                "UPDATE block_definitions SET agent='agent:foreign/name:latest'",
                [],
            )
            .unwrap();
        }
        drop(db);
        let bytes = fs::read(&definition.agents[0].history_path).unwrap();
        assert!(store
            .rename_workspace(&definition.id, &saved.revision, "new", &definition)
            .is_err());
        assert_eq!(fs::read(&definition.agents[0].history_path).unwrap(), bytes);
        assert_eq!(
            fs::read_to_string(saved.path)
                .unwrap()
                .contains("name: old"),
            true
        );
        assert!(!f.root.join("new").exists());
        let recovered = FileWorkspaceCatalogStore::new(&f.root).unwrap();
        assert!(recovered.list().unwrap()[0].error.is_none());
    }
}
