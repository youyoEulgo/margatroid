use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::{AddressError, ParsedAddress};

pub fn resolve_file(root: &Path, address: &str) -> Result<PathBuf, AddressError> {
    let address = ParsedAddress::parse(address)?;
    let entry = match address.kind.as_str() {
        "driver" => "driver.toml",
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
