use super::*;
use serde::Deserialize;
#[cfg(test)]
include!("operation_tests.rs");

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    version: u32,
    kind: String,
    source: String,
    target: String,
    revision: String,
    original_hash: String,
    staged_hash: String,
    phase: String,
    migrations: Vec<(String, String, String)>,
    member: Option<(String, String, bool)>,
    staged_revision: String,
}

#[cfg(test)]
thread_local! { static FAULT: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) }; }

fn checkpoint(name: &str) -> Result<(), WorkspaceError> {
    #[cfg(test)]
    if FAULT.with(|fault| {
        if fault.get() == Some(name) {
            fault.set(None);
            true
        } else {
            false
        }
    }) {
        return Err(invalid(format!("injected interruption at {name}")));
    }
    let _ = name;
    Ok(())
}

fn digest_tree(path: &Path) -> Result<String, WorkspaceError> {
    let mut hash = Sha256::new();
    fn visit(path: &Path, relative: &Path, hash: &mut Sha256) -> Result<(), WorkspaceError> {
        reject_symlinks(path).map_err(invalid)?;
        let mut entries = fs::read_dir(path)
            .map_err(invalid)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(invalid)?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let rel = relative.join(entry.file_name());
            let kind = entry.file_type().map_err(invalid)?;
            hash.update(rel.as_os_str().as_encoded_bytes());
            if kind.is_dir() {
                hash.update(b"directory");
                visit(&entry.path(), &rel, hash)?;
            } else if kind.is_file() {
                hash.update(b"file");
                let mut file = open_regular(&entry.path()).map_err(invalid)?;
                let length = file.metadata().map_err(invalid)?.len();
                hash.update(length.to_le_bytes());
                let mut buf = [0; 65536];
                loop {
                    let n = file.read(&mut buf).map_err(invalid)?;
                    if n == 0 {
                        break;
                    }
                    hash.update(&buf[..n]);
                }
            } else {
                return Err(invalid("unsupported file in workspace home"));
            }
        }
        Ok(())
    }
    visit(path, Path::new(""), &mut hash)?;
    Ok(format!("{:x}", hash.finalize()))
}

fn write_journal(op: &Path, journal: &Journal) -> Result<(), WorkspaceError> {
    let bytes = serde_yaml::to_string(journal).map_err(invalid)?;
    let temp = op.join("journal.next");
    reject_symlinks(&temp).map_err(invalid)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&temp)
        .map_err(invalid)?;
    file.write_all(bytes.as_bytes()).map_err(invalid)?;
    file.sync_all().map_err(invalid)?;
    fs::rename(temp, op.join("journal.yaml")).map_err(invalid)?;
    sync_directory(op)
}

fn copy_home(source: &Path, staged: &Path) -> Result<(), WorkspaceError> {
    fs::create_dir(staged).map_err(invalid)?;
    for entry in fs::read_dir(source).map_err(invalid)? {
        let entry = entry.map_err(invalid)?;
        reject_symlinks(&entry.path()).map_err(invalid)?;
        let target = staged.join(entry.file_name());
        let kind = entry.file_type().map_err(invalid)?;
        if kind.is_dir() {
            copy_home(&entry.path(), &target)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), &target).map_err(invalid)?;
            File::open(target)
                .and_then(|f| f.sync_all())
                .map_err(invalid)?;
        } else {
            return Err(invalid("unsupported workspace file"));
        }
    }
    sync_directory(staged)
}

fn complete_delete(root: &Path, op: &Path, journal: &mut Journal) -> Result<(), WorkspaceError> {
    let source = root.join(&journal.source);
    let trash = root.join(".trash");
    let destination = trash.join(&journal.target);
    for path in [&source, &trash, &destination] {
        reject_symlinks(path).map_err(invalid)?;
    }
    fs::create_dir_all(&trash).map_err(invalid)?;
    sync_directory(root)?;
    if source.exists() {
        if destination.exists() || digest_tree(&source)? != journal.original_hash {
            return Err(invalid("delete source changed or trash conflict"));
        }
        fs::rename(&source, &destination).map_err(invalid)?;
        sync_directory(root)?;
        sync_directory(&trash)?;
        checkpoint("delete_moved")?;
    } else if digest_tree(&destination)? != journal.original_hash {
        return Err(invalid("removed home checksum mismatch"));
    }
    journal.phase = "complete".into();
    write_journal(op, journal)
}

pub(super) fn delete(
    store: &FileWorkspaceCatalogStore,
    name: &str,
    expected: &str,
) -> Result<PathBuf, WorkspaceError> {
    let original = store.root.join(name);
    let original_hash = digest_tree(&original)?;
    let operations = store.root.join(".operations");
    reject_symlinks(&operations).map_err(invalid)?;
    fs::create_dir_all(&operations).map_err(invalid)?;
    sync_directory(&store.root)?;
    let op = operations.join(format!("op-{}", unique_suffix()));
    fs::create_dir(&op).map_err(invalid)?;
    sync_directory(&operations)?;
    let mut journal = Journal {
        version: 1,
        kind: "delete".into(),
        source: name.into(),
        target: format!("{name}.{}", unique_suffix()),
        revision: expected.into(),
        original_hash,
        staged_hash: String::new(),
        phase: "prepared".into(),
        migrations: Vec::new(),
        member: None,
        staged_revision: String::new(),
    };
    write_journal(&op, &journal)?;
    checkpoint("delete_prepared")?;
    if revision(&source_bytes(&original.join("workspace.yaml"))?) != expected
        || digest_tree(&original)? != journal.original_hash
    {
        return Err(conflict());
    }
    journal.phase = "committed".into();
    write_journal(&op, &journal)?;
    checkpoint("delete_committed")?;
    complete_delete(&store.root, &op, &mut journal)
        .map_err(|error| invalid(format!("commit_unknown; recovery_required: {error}")))?;
    Ok(store.root.join(".trash").join(journal.target))
}

fn member_hash(home: &Path, member: &str, had_data: bool) -> Result<String, WorkspaceError> {
    let path = home.join("agents").join(member);
    if had_data {
        digest_tree(&path)
    } else if path.exists() {
        Err(invalid("member data unexpectedly exists"))
    } else {
        Ok(revision(b""))
    }
}

fn complete_member(root: &Path, op: &Path, journal: &mut Journal) -> Result<(), WorkspaceError> {
    let (old, new, had_data) = journal
        .member
        .as_ref()
        .ok_or_else(|| invalid("member plan missing"))?;
    let home = root.join(&journal.source);
    let agents = home.join("agents");
    let original = agents.join(old);
    let target = agents.join(new);
    let backup = op.join("backup");
    let staged = op.join("staging");
    let prepared = staged.join("agents").join(new);
    for path in [&home, &original, &target, &backup, &prepared] {
        reject_symlinks(path).map_err(invalid)?;
    }
    let current_revision = revision(&source_bytes(&home.join("workspace.yaml"))?);
    if current_revision != journal.revision && current_revision != journal.staged_revision {
        return Err(invalid("member YAML changed during commit"));
    }
    if *had_data {
        if !backup.exists() {
            if target.exists() || member_hash(&home, old, true)? != journal.original_hash {
                return Err(invalid("member source changed during commit"));
            }
            fs::rename(&original, &backup).map_err(invalid)?;
            sync_directory(&agents)?;
            sync_directory(op)?;
            checkpoint("backup")?;
        } else if original.exists() || digest_tree(&backup)? != journal.original_hash {
            return Err(invalid("member backup mismatch"));
        }
        if prepared.exists() {
            if target.exists() || digest_tree(&prepared)? != journal.staged_hash {
                return Err(invalid("member target conflict or checksum mismatch"));
            }
            fs::rename(&prepared, &target).map_err(invalid)?;
            sync_directory(&agents)?;
            sync_directory(&staged.join("agents"))?;
            checkpoint("member_directory")?;
        } else if digest_tree(&target)? != journal.staged_hash {
            return Err(invalid("installed member checksum mismatch"));
        }
    } else if original.exists() || target.exists() {
        return Err(invalid("member data unexpectedly exists"));
    }
    let yaml = staged.join("workspace.yaml");
    if yaml.exists() {
        if revision(&source_bytes(&yaml)?) != journal.staged_revision {
            return Err(invalid("staged member YAML mismatch"));
        }
        fs::rename(&yaml, home.join("workspace.yaml")).map_err(invalid)?;
        sync_directory(&home)?;
        sync_directory(&staged)?;
        checkpoint("published")?;
    } else if current_revision != journal.staged_revision {
        return Err(invalid("installed member YAML mismatch"));
    }
    if member_hash(&home, new, *had_data)? != journal.staged_hash
        || revision(&source_bytes(&home.join("workspace.yaml"))?) != journal.staged_revision
    {
        return Err(invalid("final member publication verification failed"));
    }
    journal.phase = "complete".into();
    write_journal(op, journal)
}

fn complete(root: &Path, op: &Path, journal: &mut Journal) -> Result<(), WorkspaceError> {
    if journal.kind == "member" {
        return complete_member(root, op, journal);
    }
    let source = root.join(&journal.source);
    let target = root.join(&journal.target);
    let backup = op.join("backup");
    let staged = op.join("staging");
    for path in [&source, &target, &backup, &staged] {
        reject_symlinks(path).map_err(invalid)?;
    }
    if staged.exists() {
        if digest_tree(&staged)? != journal.staged_hash {
            return Err(invalid("staged home checksum mismatch"));
        }
        if !backup.exists() {
            if digest_tree(&source)? != journal.original_hash {
                return Err(invalid("source home changed during migration"));
            }
            if source != target && target.exists() {
                return Err(exists());
            }
            fs::rename(&source, &backup).map_err(invalid)?;
            sync_directory(root)?;
            sync_directory(op)?;
            checkpoint("backup")?;
        } else if digest_tree(&backup)? != journal.original_hash {
            return Err(invalid("backup checksum mismatch"));
        }
        if target.exists() {
            return Err(invalid("migration destination unexpectedly exists"));
        }
        fs::rename(&staged, &target).map_err(invalid)?;
        sync_directory(root)?;
        sync_directory(op)?;
        checkpoint("published")?;
    } else if !backup.exists()
        || digest_tree(&backup)? != journal.original_hash
        || digest_tree(&target)? != journal.staged_hash
    {
        return Err(invalid("committed migration cannot be verified"));
    }
    journal.phase = "complete".into();
    write_journal(op, journal)
}

pub(super) fn recover(root: &Path) -> Result<(), WorkspaceError> {
    let operations = root.join(".operations");
    reject_symlinks(&operations).map_err(invalid)?;
    if !operations.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(&operations).map_err(invalid)? {
        let op = entry.map_err(invalid)?.path();
        reject_symlinks(&op).map_err(invalid)?;
        if !op.is_dir() {
            return Err(invalid("unknown operation entry"));
        }
        let path = op.join("journal.yaml");
        let bytes = source_bytes(&path)?;
        let mut journal: Journal = serde_yaml::from_slice(&bytes).map_err(invalid)?;
        validate_logical_name(&journal.source, "workspace").map_err(invalid)?;
        if journal.source.starts_with('.') {
            return Err(invalid("reserved journal workspace"));
        }
        workspace_id(&journal.source)?;
        if journal.kind == "delete" {
            let prefix = format!("{}.", journal.source);
            let suffix = journal
                .target
                .strip_prefix(&prefix)
                .ok_or_else(|| invalid("invalid trash target"))?;
            if suffix.is_empty()
                || !suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || byte == b'-')
            {
                return Err(invalid("invalid trash target suffix"));
            }
        } else {
            validate_logical_name(&journal.target, "workspace").map_err(invalid)?;
            if journal.target.starts_with('.') {
                return Err(invalid("reserved journal target"));
            }
            workspace_id(&journal.target)?;
            if (journal.kind == "member") != (journal.source == journal.target) {
                return Err(invalid("inconsistent journal operation"));
            }
        }
        for (name, old, new) in &journal.migrations {
            validate_logical_name(name, "agent").map_err(invalid)?;
            let old_expected = ResourceID::new("agent", &journal.source, name, None::<String>)
                .map_err(invalid)?
                .to_string();
            let new_id = ResourceID::parse(new).map_err(invalid)?;
            let new_expected =
                ResourceID::new("agent", &journal.target, new_id.name(), None::<String>)
                    .map_err(invalid)?
                    .to_string();
            if old != &old_expected
                || new != &new_expected
                || (journal.kind == "rename" && new_id.name() != name)
                || journal.kind == "delete"
            {
                return Err(invalid("unknown journal identity mapping"));
            }
        }
        match (&journal.member, journal.kind.as_str()) {
            (Some((old, new, _)), "member") => {
                validate_logical_name(old, "agent").map_err(invalid)?;
                validate_logical_name(new, "agent").map_err(invalid)?;
                if old == new {
                    return Err(invalid("unchanged journal member"));
                }
            }
            (None, "rename" | "delete") => {}
            _ => return Err(invalid("unknown member operation plan")),
        }
        let valid_hash =
            |hash: &str| hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit());
        if journal.version != 1
            || !matches!(journal.kind.as_str(), "rename" | "member" | "delete")
            || !valid_hash(&journal.revision)
            || !valid_hash(&journal.original_hash)
        {
            return Err(invalid("unknown operation journal"));
        }
        match journal.phase.as_str() {
            "preparing" | "prepared" => {
                let home = root.join(&journal.source);
                let source_hash = if let Some((old, _, had_data)) = &journal.member {
                    member_hash(&home, old, *had_data)?
                } else {
                    digest_tree(&home)?
                };
                if op.join("backup").exists()
                    || source_hash != journal.original_hash
                    || revision(&source_bytes(&home.join("workspace.yaml"))?) != journal.revision
                {
                    return Err(invalid("uncommitted migration source unavailable"));
                }
                journal.phase = "rolled_back".into();
                write_journal(&op, &journal)?;
            }
            "committed" => {
                if journal.kind == "delete" {
                    complete_delete(root, &op, &mut journal)?;
                } else {
                    if !valid_hash(&journal.staged_hash) {
                        return Err(invalid("invalid committed checksum"));
                    }
                    complete(root, &op, &mut journal)?;
                }
            }
            "complete" | "rolled_back" => {}
            _ => return Err(invalid("unknown journal phase")),
        }
    }
    Ok(())
}

pub(super) fn migrate(
    store: &FileWorkspaceCatalogStore,
    old: &str,
    new: &str,
    expected: &str,
    source: &str,
    member: Option<(&str, &str)>,
) -> Result<(), WorkspaceError> {
    let original = store.root.join(old);
    let member_plan = member.map(|(old_member, new_member)| {
        (
            old_member.to_owned(),
            new_member.to_owned(),
            original.join("agents").join(old_member).exists(),
        )
    });
    let original_hash = if let Some((old_member, _, had_data)) = &member_plan {
        member_hash(&original, old_member, *had_data)?
    } else {
        digest_tree(&original)?
    };
    let operations = store.root.join(".operations");
    reject_symlinks(&operations).map_err(invalid)?;
    fs::create_dir_all(&operations).map_err(invalid)?;
    sync_directory(&store.root)?;
    let op = operations.join(format!("op-{}", unique_suffix()));
    fs::create_dir(&op).map_err(invalid)?;
    sync_directory(&operations)?;
    let mut journal = Journal {
        version: 1,
        kind: if member.is_some() {
            "member".into()
        } else {
            "rename".into()
        },
        source: old.into(),
        target: new.into(),
        revision: expected.into(),
        original_hash,
        staged_hash: String::new(),
        phase: "preparing".into(),
        migrations: Vec::new(),
        member: member_plan,
        staged_revision: String::new(),
    };
    write_journal(&op, &journal)?;
    checkpoint("preparing")?;
    let staged = op.join("staging");
    if let Some((old_member, _, had_data)) = &journal.member {
        fs::create_dir(&staged).map_err(invalid)?;
        if *had_data {
            fs::create_dir(staged.join("agents")).map_err(invalid)?;
            copy_home(
                &original.join("agents").join(old_member),
                &staged.join("agents").join(old_member),
            )?;
            sync_directory(&staged.join("agents"))?;
        }
        fs::copy(original.join("workspace.yaml"), op.join("source.yaml")).map_err(invalid)?;
        File::open(op.join("source.yaml"))
            .and_then(|file| file.sync_all())
            .map_err(invalid)?;
        sync_directory(&op)?;
    } else {
        copy_home(&original, &staged)?;
    }
    let agents = staged.join("agents");
    if agents.exists() {
        for entry in fs::read_dir(&agents).map_err(invalid)? {
            let entry = entry.map_err(invalid)?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| invalid("non UTF-8 member directory"))?;
            validate_logical_name(&name, "agent").map_err(invalid)?;
            if !entry.file_type().map_err(invalid)?.is_dir() {
                return Err(invalid("unknown agent home entry"));
            }
            if member.is_some_and(|(old_member, _)| old_member != name) {
                continue;
            }
            let new_member = member.map(|(_, new)| new).unwrap_or(&name);
            let database = entry.path().join("state.sqlite");
            if database.exists() {
                let old_id = ResourceID::new("agent", old, &name, None::<String>)
                    .map_err(invalid)?
                    .to_string();
                let new_id = ResourceID::new("agent", new, new_member, None::<String>)
                    .map_err(invalid)?
                    .to_string();
                let prepared = entry.path().join("state.migrated.sqlite");
                if prepared.exists() {
                    return Err(invalid("reserved migration file already exists"));
                }
                memory_plugin::prepare_agent_identity_copy(
                    &original.join("agents").join(&name).join("state.sqlite"),
                    &prepared,
                    &old_id,
                    &new_id,
                )
                .map_err(invalid)?;
                fs::rename(&prepared, &database).map_err(invalid)?;
                for suffix in ["-wal", "-shm", "-journal"] {
                    let sidecar = entry.path().join(format!("state.sqlite{suffix}"));
                    if sidecar.exists() {
                        fs::remove_file(&sidecar).map_err(invalid)?;
                    }
                }
                sync_directory(&entry.path())?;
                journal.migrations.push((name.clone(), old_id, new_id));
                write_journal(&op, &journal)?;
                checkpoint("database")?;
            }
        }
        if let Some((old_member, new_member)) = member {
            if agents.join(new_member).exists() {
                return Err(invalid("target member data already exists"));
            }
            if agents.join(old_member).exists() {
                fs::rename(agents.join(old_member), agents.join(new_member)).map_err(invalid)?;
                sync_directory(&agents)?;
            }
        }
    }
    let mut file = File::create(staged.join("workspace.yaml")).map_err(invalid)?;
    file.write_all(source.as_bytes()).map_err(invalid)?;
    file.sync_all().map_err(invalid)?;
    sync_directory(&staged)?;
    journal.staged_revision = revision(source.as_bytes());
    journal.staged_hash = if let Some((_, new_member, had_data)) = &journal.member {
        member_hash(&staged, new_member, *had_data)?
    } else {
        digest_tree(&staged)?
    };
    journal.phase = "prepared".into();
    write_journal(&op, &journal)?;
    checkpoint("prepared")?;
    let current_hash = if let Some((old_member, _, had_data)) = &journal.member {
        member_hash(&original, old_member, *had_data)?
    } else {
        digest_tree(&original)?
    };
    if revision(&source_bytes(&original.join("workspace.yaml"))?) != expected
        || current_hash != journal.original_hash
    {
        return Err(conflict());
    }
    if old != new && store.root.join(new).exists() {
        return Err(exists());
    }
    journal.phase = "committed".into();
    write_journal(&op, &journal)?;
    checkpoint("committed")?;
    complete(&store.root, &op, &mut journal)
        .map_err(|error| invalid(format!("commit_unknown; recovery_required: {error}")))
}
