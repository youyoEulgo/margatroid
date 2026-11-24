use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::{AddressError, ParsedAddress};

pub fn resolve_file(root: &Path, address: &str) -> Result<PathBuf, AddressError> {
    let address = ParsedAddress::parse(address)?;
    let entry = match address.kind.as_str() {
        "driver" => "base.lua",
        "tool" => "tool.toml",
        "prompt" => "prompt.toml",
        "sandbox" => "policy.json",
        _ => return Err(AddressError::InvalidPart),
    };
    let root = canonicalize(root)?;
    let mut candidate = root.clone();
    for part in [address.scope.as_str(), &address.name, &address.tag, entry] {
        candidate.push(part);
        candidate = canonicalize(&candidate)?;
        if !candidate.starts_with(&root) {
            return Err(AddressError::PathOutsideRoot {
                root,
                path: candidate,
            });
        }
    }
    if !fs::metadata(&candidate)?.is_file() {
        return Err(AddressError::NotFile(candidate));
    }
    Ok(candidate)
}

fn canonicalize(path: &Path) -> Result<PathBuf, AddressError> {
    fs::canonicalize(path).map_err(|error| match error.kind() {
        io::ErrorKind::NotFound => AddressError::FileMissing(path.to_owned()),
        _ => AddressError::Io(error),
    })
}

/// Enumerate the current files once. Call from a blocking worker in async flows.
pub fn query_resources(
    roots: &crate::ResourceRoots,
    query: &crate::ResourceQuery,
) -> Result<Vec<crate::ResourceFile>, AddressError> {
    query.validate()?;
    let mut result = Vec::new();
    for kind in ["driver", "tool", "prompt", "sandbox"] {
        if query
            .resource_type
            .as_deref()
            .is_some_and(|filter| filter != kind)
        {
            continue;
        }
        let configured = roots.root(kind)?;
        let root = match fs::canonicalize(configured) {
            Ok(root) => root,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        for (scope, scope_dir) in resource_directories(&root, &root, query.scope.as_deref())? {
            for (name, name_dir) in resource_directories(&root, &scope_dir, query.name.as_deref())?
            {
                for (tag, _) in resource_directories(&root, &name_dir, None)? {
                    let address =
                        match ParsedAddress::parse(&format!("{kind}:{scope}/{name}:{tag}")) {
                            Ok(v) => v,
                            Err(_) => continue,
                        };
                    let path = match resolve_file(&root, &address.to_string()) {
                        Ok(path) => path,
                        Err(
                            AddressError::FileMissing(_)
                            | AddressError::NotFile(_)
                            | AddressError::PathOutsideRoot { .. },
                        ) => continue,
                        Err(e) => return Err(e),
                    };
                    let tool_path = match kind {
                        "tool" => Some(path.clone()),
                        "driver" => {
                            let locator = format!("tool:{scope}/{name}:{tag}");
                            match resolve_file(&root, &locator) {
                                Ok(path) => Some(path),
                                Err(
                                    AddressError::FileMissing(_)
                                    | AddressError::NotFile(_)
                                    | AddressError::PathOutsideRoot { .. },
                                ) => None,
                                Err(e) => return Err(e),
                            }
                        }
                        _ => None,
                    };
                    result.push(crate::ResourceFile {
                        address,
                        path,
                        tool_path,
                    });
                }
            }
        }
    }
    result.sort_by(|a, b| {
        (
            &a.address.kind,
            &a.address.scope,
            &a.address.name,
            &a.address.tag,
        )
            .cmp(&(
                &b.address.kind,
                &b.address.scope,
                &b.address.name,
                &b.address.tag,
            ))
    });
    Ok(result)
}
fn resource_directories(
    root: &Path,
    parent: &Path,
    filter: Option<&str>,
) -> Result<Vec<(String, PathBuf)>, AddressError> {
    let candidates = if let Some(name) = filter {
        vec![(name.to_owned(), parent.join(name))]
    } else {
        let mut entries = Vec::new();
        for entry in fs::read_dir(parent)? {
            let entry = entry?;
            if let Some(name) = entry.file_name().to_str() {
                entries.push((name.to_owned(), entry.path()));
            }
        }
        entries
    };
    let mut directories = Vec::new();
    for (name, path) in candidates {
        // Validate a resource segment before accessing its descendants.
        if ParsedAddress::parse(&format!("tool:{name}/placeholder:dev")).is_err() {
            continue;
        }
        let canonical = match fs::canonicalize(&path) {
            Ok(path) => path,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        if canonical.starts_with(root) && fs::metadata(&canonical)?.is_dir() {
            directories.push((name, canonical));
        }
    }
    directories.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(directories)
}
