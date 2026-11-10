use super::*;

fn marker_path(root: &Path, mapping: &Mapping) -> PathBuf {
    root.join(format!(
        "publish_{}_{}_{}.json",
        hex(&mapping.instance),
        hex(&mapping.name),
        mapping.id
    ))
}

pub(super) fn begin(root: &Path, mapping: &Mapping) -> Result<(), StorageError> {
    let destination = block_path(root, mapping)?;
    if destination.exists() {
        return Err(write_err("Block publication destination already exists"));
    }
    fs::create_dir_all(root).map_err(write_err)?;
    let marker = marker_path(root, mapping);
    let pending = marker.with_extension("pending");
    let prepare = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&pending)
            .map_err(write_err)?;
        file.write_all(&serde_json::to_vec(mapping).map_err(write_err)?)
            .map_err(write_err)?;
        file.sync_all().map_err(write_err)?;
        if marker.exists() {
            return Err(write_err("Block publication marker exists"));
        }
        fs::rename(&pending, &marker).map_err(write_err)?;
        sync_directory(root)
    })();
    if prepare.is_err() {
        if pending.exists() {
            remove_owned_file(&pending)?;
        }
        return prepare;
    }
    fs::create_dir_all(parent(&destination)).map_err(write_err)?;
    // Persist every newly created directory before publishing a file inside it.
    let mut directory = parent(&destination);
    while directory.starts_with(root) {
        sync_directory(directory)?;
        if directory == root {
            break;
        }
        directory = parent(directory);
    }
    Ok(())
}

pub(super) fn finish(root: &Path, mapping: &Mapping) -> Result<(), StorageError> {
    let marker = marker_path(root, mapping);
    fs::remove_file(marker).map_err(|e| err(K::CommitUnknown, e))?;
    sync_directory(root)
}

fn remove_owned_file(path: &Path) -> Result<(), StorageError> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(schema_err("invalid unpublished Block file"));
        }
        fs::remove_file(path).map_err(|e| err(K::CleanupFailed, e))?;
    }
    Ok(())
}

pub(super) fn remove_database(path: &Path) -> Result<(), StorageError> {
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let mut name = path.as_os_str().to_os_string();
        name.push(suffix);
        remove_owned_file(Path::new(&name))?;
    }
    Ok(())
}

pub(super) fn abandon(root: &Path, mapping: &Mapping) -> Result<(), StorageError> {
    let path = block_path(root, mapping)?;
    // Only paths proven owned by this durable publication record are eligible.
    remove_database(&path)?;
    finish(root, mapping)
}

pub(super) fn recover(db: &Connection, root: &Path) -> Result<(), StorageError> {
    if !root.exists() {
        return Ok(());
    }
    if fs::symlink_metadata(root)
        .map_err(read_err)?
        .file_type()
        .is_symlink()
    {
        return Err(schema_err("symlink Block root"));
    }
    for entry in fs::read_dir(root).map_err(read_err)? {
        let entry = entry.map_err(read_err)?;
        let filename = entry.file_name();
        let Some(filename) = filename.to_str() else {
            continue;
        };
        if filename.starts_with("publish_") && filename.ends_with(".pending") {
            remove_owned_file(&entry.path())?;
            sync_directory(root)?;
            continue;
        }
        if !filename.starts_with("publish_") || !filename.ends_with(".json") {
            continue;
        }
        let metadata = entry.file_type().map_err(read_err)?;
        if !metadata.is_file() || metadata.is_symlink() {
            return Err(schema_err("invalid Block publication marker"));
        }
        let mapping: Mapping = serde_json::from_slice(&fs::read(entry.path()).map_err(read_err)?)
            .map_err(schema_err)?;
        valid_name(&mapping.name)?;
        validate_fields(&mapping.fields)?;
        block_path(root, &mapping)?;
        if mapping.id < 0 || mapping.intent.is_some() || marker_path(root, &mapping) != entry.path()
        {
            return Err(schema_err("invalid publication identity"));
        }
        if let Some(stored) = mappings(db)?.into_iter().find(|m| {
            m.name == mapping.name && m.instance == mapping.instance && m.id == mapping.id
        }) {
            if stored.file != mapping.file || stored.fields != mapping.fields {
                return Err(schema_err("publication mapping conflict"));
            }
            // Commit happened: preserve the published file and remove only its stale marker.
            validate_file(&block_path(root, &stored)?, &stored.fields)?;
            finish(root, &mapping)?;
        } else {
            abandon(root, &mapping)?;
        }
    }
    Ok(())
}
