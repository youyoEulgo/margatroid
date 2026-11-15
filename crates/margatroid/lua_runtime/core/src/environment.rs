//! One Lua environment constructor used by automatic and on-demand executions.
use crate::host::{HostDefinition, Margatroid};
use mlua::{AnyUserData, Lua, LuaOptions, StdLib};
pub fn create_environment(
    definition: HostDefinition,
    memory_limit: usize,
) -> mlua::Result<(Lua, AnyUserData)> {
    let lua = Lua::new_with(
        StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8,
        LuaOptions::default(),
    )?;
    lua.set_memory_limit(memory_limit)?;
    for name in ["io", "os", "print"] {
        lua.globals().set(name, mlua::Value::Nil)?;
    }
    let host = Margatroid::install(&lua, definition)?;
    lua.globals().set("coroutine", mlua::Value::Nil)?;
    Ok((lua, host))
}

pub fn configuration_globals(lua: &Lua) -> mlua::Result<Vec<(&'static str, mlua::Value)>> {
    let globals = lua.globals();
    let mut saved = Vec::new();
    for name in [
        "pcall",
        "xpcall",
        "load",
        "setmetatable",
        "dofile",
        "loadfile",
        "warn",
    ] {
        saved.push((name, globals.get::<mlua::Value>(name)?));
        globals.set(name, mlua::Value::Nil)?;
    }
    Ok(saved)
}
pub fn enable_execution(lua: &Lua, saved: Vec<(&'static str, mlua::Value)>) -> mlua::Result<()> {
    lua.load_std_libs(StdLib::IO | StdLib::OS | StdLib::COROUTINE)?;
    for (name, value) in saved {
        lua.globals().set(name, value)?;
    }
    let package = lua.create_table()?;
    package.set(
        "config",
        format!("{}\n;\n?\n!\n-\n", std::path::MAIN_SEPARATOR),
    )?;
    lua.globals().set("package", package)?;
    Ok(())
}
pub fn load_program(lua: &Lua, source: &str, origin: &std::path::Path) -> mlua::Result<()> {
    lua.load(source)
        .set_name(format!("@{}", origin.display()))
        .exec()
}
