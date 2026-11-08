//! Pure regular-expression search over byte strings.
use mlua::{Lua, LuaSerdeExt, Table};

pub(super) fn install(lua: &Lua, callbacks: &Table) -> mlua::Result<()> {
    callbacks.set(
        "text_search",
        lua.create_function(|lua, (pattern, content): (String, mlua::String)| {
            let regex = regex::bytes::Regex::new(&pattern)
                .map_err(|err| super::error(format!("invalid regular expression: {err}")))?;
            let matches = lua.create_table()?;
            matches.set_metatable(Some(lua.array_metatable()))?;
            for (index, found) in regex.find_iter(&content.as_bytes()).enumerate() {
                let value = lua.create_table_with_capacity(0, 2)?;
                value.raw_set("start", found.start())?;
                value.raw_set("end", found.end())?;
                matches.raw_set(index + 1, value)?;
            }
            Ok(matches)
        })?,
    )
}
