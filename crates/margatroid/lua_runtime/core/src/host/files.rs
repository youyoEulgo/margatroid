//! Filesystem operations and pure path glob matching for the current worker.
use super::{error, io_error};
use mlua::{Lua, LuaSerdeExt, Table};
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

// Unix paths are byte strings. Other platforms deliberately require UTF-8.
fn path_from_lua(value: &mlua::String) -> mlua::Result<PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Ok(PathBuf::from(OsStr::from_bytes(&value.as_bytes())))
    }
    #[cfg(not(unix))]
    {
        Ok(PathBuf::from(value.to_str()?.as_ref()))
    }
}
fn os_to_lua(lua: &Lua, value: &OsStr) -> mlua::Result<mlua::String> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        lua.create_string(value.as_bytes())
    }
    #[cfg(not(unix))]
    {
        lua.create_string(
            value
                .to_str()
                .ok_or_else(|| error("filesystem path is not valid UTF-8"))?,
        )
    }
}
fn kind(value: std::fs::FileType) -> &'static str {
    if value.is_symlink() {
        "symlink"
    } else if value.is_file() {
        "file"
    } else if value.is_dir() {
        "directory"
    } else {
        "other"
    }
}
fn modified_at_ns(meta: &std::fs::Metadata) -> Option<i64> {
    let time = meta.modified().ok()?;
    let ns = match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => i128::try_from(duration.as_nanos()).ok()?,
        Err(err) => -i128::try_from(err.duration().as_nanos()).ok()?,
    };
    i64::try_from(ns).ok()
}
async fn info(lua: &Lua, path: &Path) -> mlua::Result<Table> {
    let meta = tokio::fs::symlink_metadata(path)
        .await
        .map_err(|e| io_error("files_info", e))?;
    let resolved = tokio::fs::canonicalize(path).await.ok();
    let table = lua.create_table()?;
    table.raw_set("path", os_to_lua(lua, path.as_os_str())?)?;
    if let Some(resolved) = resolved {
        table.raw_set("resolved_path", os_to_lua(lua, resolved.as_os_str())?)?;
    }
    table.raw_set("kind", kind(meta.file_type()))?;
    table.raw_set("size", meta.len())?;
    table.raw_set("modified_at_ns", modified_at_ns(&meta))?;
    Ok(table)
}

pub(super) fn install(lua: &Lua, callbacks: &Table, root: PathBuf) -> mlua::Result<()> {
    let root = if root.is_absolute() {
        root
    } else {
        std::env::current_dir()
            .map_err(|e| io_error("project root", e))?
            .join(root)
    };
    let project = root.clone();
    callbacks.set(
        "files_read",
        lua.create_async_function(move |lua, name: mlua::String| {
            let path = path_from_lua(&name).map(|path| project.join(path));
            async move {
                let bytes = tokio::fs::read(path?)
                    .await
                    .map_err(|e| io_error("files_read", e))?;
                lua.create_string(bytes)
            }
        })?,
    )?;
    let project = root.clone();
    callbacks.set(
        "files_write",
        lua.create_async_function(move |_, (name, bytes): (mlua::String, mlua::String)| {
            let path = path_from_lua(&name).map(|path| project.join(path));
            async move {
                tokio::fs::write(path?, &*bytes.as_bytes())
                    .await
                    .map_err(|e| io_error("files_write", e))
            }
        })?,
    )?;
    let project = root.clone();
    callbacks.set(
        "files_info",
        lua.create_async_function(move |lua, name: mlua::String| {
            let path = path_from_lua(&name).map(|path| project.join(path));
            async move { info(&lua, &path?).await }
        })?,
    )?;
    let project = root.clone();
    callbacks.set(
        "files_list",
        lua.create_async_function(move |lua, name: mlua::String| {
            let path = path_from_lua(&name).map(|path| project.join(path));
            async move {
                let mut dir = tokio::fs::read_dir(path?)
                    .await
                    .map_err(|e| io_error("files_list", e))?;
                let mut entries = Vec::new();
                while let Some(entry) = dir
                    .next_entry()
                    .await
                    .map_err(|e| io_error("files_list", e))?
                {
                    entries.push((entry.file_name(), entry.path()));
                }
                entries.sort_by(|a, b| a.0.cmp(&b.0));
                let table = lua.create_table_with_capacity(entries.len(), 0)?;
                table.set_metatable(Some(lua.array_metatable()))?;
                for (index, (name, path)) in entries.into_iter().enumerate() {
                    let entry = info(&lua, &path).await?;
                    entry.raw_set("name", os_to_lua(&lua, &name)?)?;
                    table.raw_set(index + 1, entry)?;
                }
                Ok(table)
            }
        })?,
    )?;
    let project = root.clone();
    callbacks.set(
        "files_create_directory",
        lua.create_async_function(move |_, (name, recursive): (mlua::String, bool)| {
            let path = path_from_lua(&name).map(|path| project.join(path));
            async move {
                let path = path?;
                let result = if recursive {
                    tokio::fs::create_dir_all(&path).await
                } else {
                    tokio::fs::create_dir(&path).await
                };
                match result {
                    Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                        if tokio::fs::metadata(&path)
                            .await
                            .map(|m| m.is_dir())
                            .unwrap_or(false)
                        {
                            Ok(())
                        } else {
                            Err(io_error("files_create_directory", err))
                        }
                    }
                    result => result.map_err(|e| io_error("files_create_directory", e)),
                }
            }
        })?,
    )?;
    callbacks.set(
        "files_matches",
        lua.create_function(move |_, (path, pattern): (mlua::String, String)| {
            let path = path_from_lua(&path)?;
            let path = path.strip_prefix(&root).unwrap_or(&path);
            // Slashless rg globs match basenames at every depth; slash globs are rooted.
            // `!` is literal here: include/exclude policy belongs to the calling tool.
            let basename = !pattern.contains('/');
            let pattern = pattern.strip_prefix('/').unwrap_or(&pattern);
            let matcher = globset::GlobBuilder::new(pattern)
                .literal_separator(true)
                .backslash_escape(true)
                .build()
                .map_err(|e| error(format!("invalid glob: {e}")))?
                .compile_matcher();
            let candidate = if basename {
                Path::new(path.file_name().unwrap_or(path.as_os_str()))
            } else {
                path
            };
            Ok(matcher.is_match(candidate))
        })?,
    )?;
    Ok(())
}
