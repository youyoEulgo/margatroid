//! Data conversion and filesystem callbacks local to the sandboxed worker.
use super::{SharedHostError, SharedHostErrorKind as Kind};
use base64::{Engine, engine::general_purpose::STANDARD};
use mlua::{Lua, LuaSerdeExt, Value as LuaValue};
use serde_json::Value;
use std::{collections::HashSet, path::PathBuf};

#[path = "files.rs"]
mod files;
#[path = "text.rs"]
mod text;

const KIND: &str = "__margatroid_data_kind";
fn error(message: impl Into<String>) -> mlua::Error {
    mlua::Error::external(SharedHostError::new(Kind::InvalidArguments, message))
}
fn io_error(operation: &str, err: std::io::Error) -> mlua::Error {
    mlua::Error::external(SharedHostError::new(
        Kind::ExecutionFailed,
        format!("{operation}: {err}"),
    ))
}
pub fn from_lua(lua: &Lua, value: LuaValue) -> mlua::Result<Value> {
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
/// Convert object-only inputs such as log details, treating an unmarked `{}` as
/// an empty object while respecting explicitly encoded array/object identity.
pub fn from_lua_object(lua: &Lua, value: LuaValue) -> mlua::Result<Value> {
    if let LuaValue::Table(table) = &value {
        let metatable = table.metatable();
        let marker = metatable
            .as_ref()
            .and_then(|meta| meta.raw_get::<String>(KIND).ok());
        let marked_array = marker.as_deref() == Some("array")
            || metatable
                .as_ref()
                .is_some_and(|meta| meta.to_pointer() == lua.array_metatable().to_pointer());
        if marked_array {
            return Err(error("expected an object"));
        }
        if marker.is_none() && table.clone().pairs::<LuaValue, LuaValue>().next().is_none() {
            return Ok(Value::Object(serde_json::Map::new()));
        }
    }
    let value = from_lua(lua, value)?;
    if value.is_object() {
        Ok(value)
    } else {
        Err(error("expected an object"))
    }
}

pub fn to_lua(lua: &Lua, value: Value) -> mlua::Result<LuaValue> {
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
/// Callbacks are plain functions; the host userdata dispatch removes `self`.
pub fn install(lua: &Lua, callbacks: &mlua::Table, root: PathBuf) -> mlua::Result<()> {
    callbacks.set(
        "data_encode",
        lua.create_function(|lua, (format, value): (String, LuaValue)| {
            if format == "base64" {
                let LuaValue::String(bytes) = value else {
                    return Err(error("base64 encoding requires a byte string"));
                };
                return lua.create_string(STANDARD.encode(bytes.as_bytes()));
            }
            if !matches!(format.as_str(), "json" | "yaml" | "toml") {
                return Err(error("unsupported data format"));
            }
            let value = from_lua(lua, value)?;
            let bytes = match format.as_str() {
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
        "data_decode",
        lua.create_function(|lua, (format, content): (String, mlua::String)| {
            if format == "base64" {
                let bytes = STANDARD
                    .decode(content.as_bytes())
                    .map_err(mlua::Error::external)?;
                return Ok(LuaValue::String(lua.create_string(bytes)?));
            }
            if !matches!(format.as_str(), "json" | "yaml" | "toml") {
                return Err(error("unsupported data format"));
            }
            let text = content.to_str()?;
            let value = match format.as_str() {
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
    files::install(lua, callbacks, root)?;
    text::install(lua, callbacks)
}

#[cfg(test)]
#[path = "../../tests/internal/matches.rs"]
mod match_tests;
#[cfg(test)]
#[path = "../../tests/internal/data.rs"]
mod tests;
