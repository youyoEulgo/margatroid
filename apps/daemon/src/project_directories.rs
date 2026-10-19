use margatroid_types::discovery::{
    ProjectDirectoryBrowser, ProjectDirectoryEntry, ProjectDirectoryPage,
};
use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    path::PathBuf,
};

pub(crate) struct LocalProjectDirectoryBrowser {
    home: PathBuf,
}
impl LocalProjectDirectoryBrowser {
    pub(crate) fn new(home: PathBuf) -> Result<Self, String> {
        let home = home
            .canonicalize()
            .map_err(|e| format!("Cannot open home directory {}: {e}", home.display()))?;
        if !home.is_dir() {
            return Err("Home path must be a directory".into());
        }
        Ok(Self { home })
    }
}
impl ProjectDirectoryBrowser for LocalProjectDirectoryBrowser {
    fn query(
        &self,
        path: Option<PathBuf>,
        cursor: Option<String>,
    ) -> Result<ProjectDirectoryPage, String> {
        let requested = path.unwrap_or_else(|| self.home.clone());
        if !requested.is_absolute() {
            return Err("Choose an absolute directory on the daemon host".into());
        }
        let path = requested
            .canonicalize()
            .map_err(|e| format!("Cannot open directory {}: {e}", requested.display()))?;
        if !path.is_dir() {
            return Err(format!("Not a directory: {}", path.display()));
        }
        let mut entries = Vec::new();
        for (index, entry) in fs::read_dir(&path)
            .map_err(|e| format!("Cannot list directory {}: {e}", path.display()))?
            .enumerate()
        {
            if index >= 20_000 {
                return Err(
                    "Directory contains too many entries; enter a more specific path".into(),
                );
            }
            let entry = entry.map_err(|e| format!("Cannot read directory entry: {e}"))?;
            let kind = entry
                .file_type()
                .map_err(|e| format!("Cannot inspect directory entry: {e}"))?;
            if !kind.is_dir() {
                continue;
            }
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let canonical = entry
                .path()
                .canonicalize()
                .map_err(|e| format!("Directory changed while reading: {e}"))?;
            if canonical.parent() != Some(path.as_path()) {
                continue;
            }
            entries.push(ProjectDirectoryEntry {
                name,
                path: canonical,
            });
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        let mut hash = DefaultHasher::new();
        path.hash(&mut hash);
        for e in &entries {
            e.name.hash(&mut hash);
            e.path.hash(&mut hash);
        }
        if let Ok(modified) = fs::metadata(&path).and_then(|m| m.modified()) {
            modified.hash(&mut hash);
        }
        let version = format!("{:016x}", hash.finish());
        let offset = match cursor {
            None => 0,
            Some(cursor) => {
                let Some((old_version, offset)) = cursor.split_once(':') else {
                    return Err("Invalid directory cursor; refresh this folder".into());
                };
                let offset = offset
                    .parse::<usize>()
                    .map_err(|_| "Invalid directory cursor".to_string())?;
                if old_version != version
                    || offset == 0
                    || offset % 200 != 0
                    || offset >= entries.len()
                {
                    return Err("Directory changed or cursor expired; refresh this folder".into());
                }
                offset
            }
        };
        let next = (offset + 200 < entries.len()).then(|| format!("{version}:{}", offset + 200));
        let directories = entries.into_iter().skip(offset).take(200).collect();
        Ok(ProjectDirectoryPage {
            parent: path.parent().map(Into::into),
            path,
            directories,
            next_cursor: next,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn browsing_is_canonical_directory_only_paginated_and_versioned() {
        let root = tempfile::tempdir().unwrap();
        for i in 0..205 {
            fs::create_dir(root.path().join(format!("folder-{i:03}"))).unwrap();
        }
        fs::write(root.path().join("file.txt"), "not a folder").unwrap();
        let browser = LocalProjectDirectoryBrowser::new(root.path().into()).unwrap();
        let first = browser.query(None, None).unwrap();
        assert_eq!(first.directories.len(), 200);
        assert_eq!(first.directories[0].name, "folder-000");
        let cursor = first.next_cursor.unwrap();
        let second = browser
            .query(Some(first.path.clone()), Some(cursor.clone()))
            .unwrap();
        assert_eq!(second.directories.len(), 5);
        assert!(second.next_cursor.is_none());
        fs::create_dir(root.path().join("another-folder")).unwrap();
        assert!(browser
            .query(None, Some(cursor))
            .unwrap_err()
            .contains("changed"));
        assert!(browser.query(Some("relative".into()), None).is_err());
        assert!(browser
            .query(Some(root.path().join("file.txt")), None)
            .is_err());
        assert!(!root.path().join("missing").exists());
        assert!(browser
            .query(Some(root.path().join("missing")), None)
            .is_err());
        assert!(!root.path().join("missing").exists());
    }
    #[cfg(unix)]
    #[test]
    fn symbolic_links_are_not_mistaken_for_direct_child_folders() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("linked")).unwrap();
        let browser = LocalProjectDirectoryBrowser::new(root.path().into()).unwrap();
        assert!(browser.query(None, None).unwrap().directories.is_empty());
        let page = browser
            .query(Some(root.path().join("linked")), None)
            .unwrap();
        assert_eq!(page.path, outside.path().canonicalize().unwrap());
    }
}
