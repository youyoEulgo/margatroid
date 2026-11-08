//! The only tool host entry point exposed to Lua.
use mlua::{
    AnyUserData, Function, Lua, LuaSerdeExt, MultiValue, Table, UserData, UserDataFields,
    UserDataMethods,
};
use serde_json::Value;

use crate::types::ToolContext;

pub(crate) struct Margatroid {
    context: ToolContext,
    arguments: Value,
    callbacks: Option<Table>,
}

impl Margatroid {
    pub(crate) fn install(
        lua: &Lua,
        context: ToolContext,
        arguments: Value,
    ) -> mlua::Result<AnyUserData> {
        let object = lua.create_userdata(Self {
            context,
            arguments,
            callbacks: None,
        })?;
        lua.globals().set("margatroid", object.clone())?;
        Ok(object)
    }

    pub(crate) fn enable(object: &AnyUserData, callbacks: Table) -> mlua::Result<()> {
        let mut host = object.borrow_mut::<Self>()?;
        if host.callbacks.is_some() {
            return Err(mlua::Error::external("margatroid methods already enabled"));
        }
        host.callbacks = Some(callbacks);
        Ok(())
    }

    fn callback(&self, name: &str) -> mlua::Result<Function> {
        self.callbacks
            .as_ref()
            .ok_or_else(|| {
                mlua::Error::external("margatroid methods unavailable during configure")
            })?
            .get(name)
    }
}

impl UserData for Margatroid {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("version", |_, _| Ok(1u32));
        fields.add_field_method_get("context", |lua, host| {
            let value = serde_json::to_value(&host.context).map_err(mlua::Error::external)?;
            super::data::to_lua(lua, value)
        });
        fields.add_field_method_get("arguments", |lua, host| {
            super::data::to_lua(lua, host.arguments.clone())
        });
        fields.add_field_method_get("null", |lua, _| Ok(lua.null()));
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        for name in [
            "task_write",
            "task_replace",
            "task_return",
            "task_finish",
            "task_read",
            "task_send",
            "task_receive",
            "tool_inject",
            "process_open",
            "await_any",
            "read_file",
            "write_file",
            "read_dir",
            "metadata",
            "canonicalize",
        ] {
            methods.add_async_function(
                name,
                move |_, (object, args): (AnyUserData, MultiValue)| {
                    // Clone the callable synchronously. No userdata borrow survives an await,
                    // including while callbacks call other methods on this same object.
                    let callback = object.borrow::<Self>().and_then(|host| host.callback(name));
                    async move { callback?.call_async::<MultiValue>(args).await }
                },
            );
        }
        for name in [
            "json_decode",
            "json_encode",
            "yaml_decode",
            "yaml_encode",
            "toml_decode",
            "toml_encode",
            "base64_encode",
            "base64_decode",
        ] {
            methods.add_function(name, move |_, (object, args): (AnyUserData, MultiValue)| {
                let callback = object.borrow::<Self>()?.callback(name)?;
                callback.call::<MultiValue>(args)
            });
        }
    }
}

#[cfg(test)]
#[path = "../../tests/internal/host_object.rs"]
mod tests;
