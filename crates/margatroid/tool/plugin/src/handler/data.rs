//! Data conversion and filesystem callbacks local to the sandboxed worker.
use base64::{engine::general_purpose::STANDARD, Engine};
use mlua::{Lua, LuaSerdeExt, Value as LuaValue};
use serde_json::Value;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tool_core::{ToolError, ToolErrorKind as Kind};

const KIND: &str = "__margatroid_data_kind";
fn error(message: impl Into<String>) -> mlua::Error {
    mlua::Error::external(ToolError::new(Kind::InvalidArguments, message))
}
fn io_error(operation: &str, err: std::io::Error) -> mlua::Error {
    mlua::Error::external(ToolError::new(
        Kind::ExecutionFailed,
        format!("{operation}: {err}"),
    ))
}
fn from_lua(lua: &Lua, value: LuaValue) -> mlua::Result<Value> {
    fn walk(lua: &Lua, value: LuaValue, seen: &mut HashSet<usize>) -> mlua::Result<Value> {
        match value {
            LuaValue::Nil => Ok(Value::Null),
            LuaValue::Number(n) if !n.is_finite() => {
                Err(error("non-finite numbers are unsupported"))
            }
            LuaValue::Table(table) => {
                let id = table.to_pointer() as usize;
                if !seen.insert(id) {
                    return Err(error("recursive table is unsupported"));
                }
                let marker = table
                    .metatable()
                    .and_then(|m| m.raw_get::<String>(KIND).ok());
                let mut entries = Vec::new();
                for pair in table.clone().pairs::<LuaValue, LuaValue>() {
                    let (key, value) = pair?;
                    entries.push((key, value));
                }
                let array = marker.as_deref() == Some("array")
                    || (marker.as_deref() != Some("object")
                        && (entries.is_empty()
                            || entries
                                .iter()
                                .all(|(k, _)| matches!(k, LuaValue::Integer(n) if *n > 0))));
                let result = if array {
                    let count = entries.len();
                    let mut values = vec![None; count];
                    for (key, value) in entries {
                        let LuaValue::Integer(index) = key else {
                            return Err(error("array contains a non-integer key"));
                        };
                        let index = usize::try_from(index)
                            .ok()
                            .and_then(|i| i.checked_sub(1))
                            .filter(|i| *i < count)
                            .ok_or_else(|| error("array keys must be contiguous starting at 1"))?;
                        values[index] = Some(walk(lua, value, seen)?);
                    }
                    Value::Array(
                        values
                            .into_iter()
                            .map(|v| v.ok_or_else(|| error("sparse array is unsupported")))
                            .collect::<mlua::Result<_>>()?,
                    )
                } else {
                    let mut values = serde_json::Map::new();
                    for (key, value) in entries {
                        let LuaValue::String(key) = key else {
                            return Err(error("object keys must be strings"));
                        };
                        let key = key.to_str()?.to_owned();
                        values.insert(key, walk(lua, value, seen)?);
                    }
                    Value::Object(values)
                };
                seen.remove(&id);
                Ok(result)
            }
            other => lua.from_value(other),
        }
    }
    walk(lua, value, &mut HashSet::new())
}
pub(super) fn to_lua(lua: &Lua, value: Value) -> mlua::Result<LuaValue> {
    match value {
        Value::Array(values) => {
            let table = lua.create_table_with_capacity(values.len(), 0)?;
            table.set_metatable(Some(lua.array_metatable()))?;
            for (index, value) in values.into_iter().enumerate() {
                table.raw_set(index + 1, to_lua(lua, value)?)?;
            }
            Ok(LuaValue::Table(table))
        }
        Value::Object(values) => {
            let table = lua.create_table_with_capacity(0, values.len())?;
            let meta = lua.create_table()?;
            meta.raw_set(KIND, "object")?;
            table.set_metatable(Some(meta))?;
            for (key, value) in values {
                table.raw_set(key, to_lua(lua, value)?)?;
            }
            Ok(LuaValue::Table(table))
        }
        value => lua.to_value(&value),
    }
}
// Preserve YAML type validation while leaving parser resource limits to serde_yaml.
struct YamlSeed;
impl<'de> serde::de::DeserializeSeed<'de> for YamlSeed {
    type Value = Value;
    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        deserializer.deserialize_any(self)
    }
}
impl<'de> serde::de::Visitor<'de> for YamlSeed {
    type Value = Value;
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("YAML data with string object keys and no tags")
    }
    fn visit_unit<E: serde::de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_none<E: serde::de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Value, E> {
        Ok(Value::Bool(v))
    }
    fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Value, E> {
        Ok(Value::from(v))
    }
    fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Value, E> {
        Ok(Value::from(v))
    }
    fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Value, E> {
        serde_json::Number::from_f64(v)
            .map(Value::Number)
            .ok_or_else(|| E::custom("non-finite numbers are unsupported"))
    }
    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Value, E> {
        Ok(Value::String(v.to_owned()))
    }
    fn visit_string<E: serde::de::Error>(self, v: String) -> Result<Value, E> {
        Ok(Value::String(v))
    }
    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = seq.next_element_seed(YamlSeed)? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }
    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut values = serde_json::Map::new();
        while let Some(key) = map.next_key_seed(YamlSeed)? {
            let Value::String(key) = key else {
                return Err(serde::de::Error::custom("YAML object keys must be strings"));
            };
            if values.contains_key(&key) {
                return Err(serde::de::Error::custom("duplicate YAML object key"));
            }
            let value = map.next_value_seed(YamlSeed)?;
            values.insert(key, value);
        }
        Ok(Value::Object(values))
    }
    fn visit_enum<A: serde::de::EnumAccess<'de>>(self, _: A) -> Result<Value, A::Error> {
        Err(serde::de::Error::custom("YAML tags are unsupported"))
    }
}
fn yaml_decode(text: &str) -> mlua::Result<Value> {
    use serde::de::DeserializeSeed;
    let mut documents = serde_yaml::Deserializer::from_str(text);
    let document = documents
        .next()
        .ok_or_else(|| error("YAML document is missing"))?;
    let value = YamlSeed
        .deserialize(document)
        .map_err(mlua::Error::external)?;
    if documents.next().is_some() {
        return Err(error("expected a single YAML document"));
    }
    Ok(value)
}
fn toml_value(value: toml::Value) -> mlua::Result<Value> {
    Ok(match value {
        toml::Value::Datetime(v) => Value::String(v.to_string()),
        toml::Value::Array(v) => {
            Value::Array(v.into_iter().map(toml_value).collect::<mlua::Result<_>>()?)
        }
        toml::Value::Table(v) => Value::Object(
            v.into_iter()
                .map(|(k, v)| Ok((k, toml_value(v)?)))
                .collect::<mlua::Result<_>>()?,
        ),
        toml::Value::Float(v) if !v.is_finite() => {
            return Err(error("non-finite numbers are unsupported"));
        }
        value => serde_json::to_value(value).map_err(mlua::Error::external)?,
    })
}
fn path(root: &Path, value: &str) -> mlua::Result<PathBuf> {
    Ok(root.join(value))
}
fn path_text(value: &Path) -> mlua::Result<String> {
    value
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| error("filesystem path is not valid UTF-8"))
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
/// Callbacks are plain functions; the host userdata dispatch removes `self`.
pub(super) fn install(
    lua: &Lua,
    callbacks: &mlua::Table,
    project_root: PathBuf,
) -> Result<(), ToolError> {
    install_inner(lua, callbacks, project_root)
        .map_err(|e| ToolError::new(Kind::ExecutionFailed, e.to_string()))
}
fn install_inner(lua: &Lua, callbacks: &mlua::Table, root: PathBuf) -> mlua::Result<()> {
    for format in ["json", "yaml", "toml"] {
        callbacks.set(
            format!("{format}_encode"),
            lua.create_function(move |lua, value: LuaValue| {
                let value = from_lua(lua, value)?;
                let bytes = match format {
                    "json" => serde_json::to_vec(&value).map_err(mlua::Error::external)?,
                    "yaml" => {
                        let mut bytes = Vec::new();
                        serde_yaml::to_writer(&mut bytes, &value).map_err(mlua::Error::external)?;
                        bytes
                    }
                    _ => {
                        if !value.is_object() {
                            return Err(error("TOML root must be an object"));
                        }
                        toml::to_string(&value)
                            .map_err(mlua::Error::external)?
                            .into_bytes()
                    }
                };

                lua.create_string(bytes)
            })?,
        )?;
        callbacks.set(
            format!("{format}_decode"),
            lua.create_function(move |lua, text: mlua::String| {
                let text = text.to_str()?;
                let value = match format {
                    "json" => serde_json::from_str(&text).map_err(mlua::Error::external)?,
                    "yaml" => yaml_decode(&text)?,
                    _ => toml_value(
                        toml::from_str::<toml::Table>(&text)
                            .map(toml::Value::Table)
                            .map_err(mlua::Error::external)?,
                    )?,
                };

                to_lua(lua, value)
            })?,
        )?;
    }
    callbacks.set(
        "base64_encode",
        lua.create_function(move |lua, bytes: mlua::String| {
            lua.create_string(STANDARD.encode(bytes.as_bytes()))
        })?,
    )?;
    callbacks.set(
        "base64_decode",
        lua.create_function(move |lua, text: mlua::String| {
            let bytes = STANDARD
                .decode(text.as_bytes())
                .map_err(mlua::Error::external)?;
            lua.create_string(bytes)
        })?,
    )?;
    let project = root.clone();
    callbacks.set(
        "read_file",
        lua.create_async_function(move |lua, name: String| {
            let path = path(&project, &name);
            async move {
                let mut file = tokio::fs::File::open(path?)
                    .await
                    .map_err(|e| io_error("read_file", e))?;
                let mut bytes = Vec::new();
                file.read_to_end(&mut bytes)
                    .await
                    .map_err(|e| io_error("read_file", e))?;

                lua.create_string(bytes)
            }
        })?,
    )?;
    let project = root.clone();
    callbacks.set(
        "write_file",
        lua.create_async_function(move |_, (name, bytes): (String, mlua::String)| {
            let path = path(&project, &name);
            async move {
                let mut file = tokio::fs::File::create(path?)
                    .await
                    .map_err(|e| io_error("write_file", e))?;
                file.write_all(&bytes.as_bytes())
                    .await
                    .map_err(|e| io_error("write_file", e))?;
                file.flush().await.map_err(|e| io_error("write_file", e))
            }
        })?,
    )?;
    let project = root.clone();
    callbacks.set(
        "read_dir",
        lua.create_async_function(move |lua, name: String| {
            let path = path(&project, &name);
            async move {
                let mut dir = tokio::fs::read_dir(path?)
                    .await
                    .map_err(|e| io_error("read_dir", e))?;
                let mut values = Vec::new();
                while let Some(entry) = dir
                    .next_entry()
                    .await
                    .map_err(|e| io_error("read_dir", e))?
                {
                    let name = entry
                        .file_name()
                        .into_string()
                        .map_err(|_| error("directory name is not valid UTF-8"))?;
                    let path = path_text(&entry.path())?;
                    let kind = kind(
                        entry
                            .file_type()
                            .await
                            .map_err(|e| io_error("read_dir", e))?,
                    );
                    let value = serde_json::json!({"name":name,"path":path,"kind":kind});
                    values.push(value);
                }
                values.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
                to_lua(&lua, Value::Array(values))
            }
        })?,
    )?;
    let project = root.clone();
    callbacks.set(
        "metadata",
        lua.create_async_function(move |lua, name: String| {
            let path = path(&project, &name);
            async move {
                let meta = tokio::fs::symlink_metadata(path?)
                    .await
                    .map_err(|e| io_error("metadata", e))?;
                let value =
                    serde_json::json!({"kind":kind(meta.file_type()),"size_bytes":meta.len()});

                to_lua(&lua, value)
            }
        })?,
    )?;
    callbacks.set(
        "canonicalize",
        lua.create_async_function(move |lua, name: String| {
            let path = path(&root, &name);
            async move {
                let value = path_text(
                    &tokio::fs::canonicalize(path?)
                        .await
                        .map_err(|e| io_error("canonicalize", e))?,
                )?;

                lua.create_string(value)
            }
        })?,
    )?;
    Ok(())
}
#[cfg(test)]
#[path = "../../tests/internal/data.rs"]
mod tests;
