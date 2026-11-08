use super::*;
use std::path::PathBuf;

fn fixture() -> (Lua, AnyUserData) {
    let lua = Lua::new();
    let host = Margatroid::install(
        &lua,
        ToolContext {
            task_id: "task".into(),
            call_id: "call".into(),
            alias: "alias".into(),
            owner_task_id: Some("owner".into()),
            binding: Some(serde_json::json!({"nested": {"value": "trusted"}, "empty_object": {}})),
            resource_dir: PathBuf::from("/resource"),
            project_root: PathBuf::from("/project"),
            temp_dir: PathBuf::from("/temp"),
        },
        serde_json::json!({"nested": {"value": 42}, "empty_object": {}, "empty_array": []}),
    )
    .unwrap();
    (lua, host)
}

#[test]
fn metadata_is_available_before_loading_and_getters_return_copies() {
    let (lua, _) = fixture();
    lua.load(
        r#"
        assert(type(margatroid) == "userdata")
        local loaded = margatroid.context
        function configure(arguments, context)
            assert(margatroid.version == 1)
            assert(loaded.owner_task_id == "owner")
            loaded.binding.nested.value = "tampered"
            local arguments = margatroid.arguments
            arguments.nested.value = 0
            assert(margatroid.context.binding.nested.value == "trusted")
            assert(margatroid.arguments.nested.value == 42)
            assert(margatroid.null ~= nil)
            return {timeout_ms=0}
        end
        configure({}, {})
        for _, field in ipairs({"version", "context", "arguments", "null"}) do
            assert(not pcall(function() margatroid[field] = {} end))
        end
        assert(not pcall(function() margatroid() end))
    "#,
    )
    .exec()
    .unwrap();
}

#[tokio::test]
async fn configure_rejects_host_methods_and_free_host_globals_are_absent() {
    let (lua, _) = fixture();
    for method in [
        "task_write",
        "task_receive",
        "tool_inject",
        "process_open",
        "await_any",
        "read_file",
        "json_decode",
    ] {
        let error = lua
            .load(format!("return margatroid:{method}({{}})"))
            .eval_async::<mlua::Value>()
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("unavailable during configure"),
            "{method}: {error}"
        );
    }
    lua.load(
        r#"
        for _, name in ipairs({"args", "context", "json", "task_write", "task_replace",
            "task_return", "task_finish", "task_read", "task_send", "task_receive",
            "tool_inject", "process_open", "await_any", "base64_encode"}) do
            assert(_G[name] == nil, name)
        end
    "#,
    )
    .exec()
    .unwrap();
}

#[tokio::test]
async fn colon_methods_forward_structured_values_and_keep_callbacks_private() {
    let (lua, host) = fixture();
    let callbacks = lua.create_table().unwrap();
    callbacks
        .set(
            "task_write",
            lua.create_async_function(|lua, value: mlua::Value| async move {
                let value: Value = lua.from_value(value)?;
                assert_eq!(value, serde_json::json!({"values": [true, null, 3]}));
                Ok(())
            })
            .unwrap(),
        )
        .unwrap();
    super::super::data::install(&lua, &callbacks, PathBuf::from("/project")).unwrap();
    Margatroid::enable(&host, callbacks).unwrap();
    lua.load(
        r#"
        assert(margatroid:json_encode(margatroid.arguments.empty_object) == '{}')
        assert(margatroid:json_encode(margatroid.arguments.empty_array) == '[]')
        assert(margatroid:json_encode(margatroid.context.binding.empty_object) == '{}')
        local value = margatroid:json_decode('{"values":[true,null,3]}')
        assert(value.values[2] == margatroid.null)
        margatroid:task_write(value)
        assert(margatroid:json_encode(value) == '{"values":[true,null,3]}')
        assert(task_write == nil and json == nil and args == nil and context == nil)
        assert(margatroid.callbacks == nil)
    "#,
    )
    .exec_async()
    .await
    .unwrap();
}

#[tokio::test]
async fn concurrent_methods_release_userdata_borrows_before_awaiting() {
    let (lua, host) = fixture();
    let callbacks = lua.create_table().unwrap();
    callbacks
        .set("await_any", super::super::install_await_any(&lua).unwrap())
        .unwrap();
    let (entered, waiting) = tokio::sync::oneshot::channel();
    let entered = std::sync::Arc::new(std::sync::Mutex::new(Some(entered)));
    callbacks
        .set(
            "task_receive",
            lua.create_async_function(move |lua, ()| {
                let entered = entered.clone();
                async move {
                    let object: AnyUserData = lua.globals().get("margatroid")?;
                    // A mutable borrow catches both shared and exclusive borrows held by dispatch.
                    drop(object.borrow_mut::<Margatroid>()?);
                    entered.lock().unwrap().take().unwrap().send(()).unwrap();
                    std::future::pending::<mlua::Result<()>>().await
                }
            })
            .unwrap(),
        )
        .unwrap();
    let waiting = std::sync::Arc::new(tokio::sync::Mutex::new(Some(waiting)));
    callbacks
        .set(
            "task_write",
            lua.create_async_function(move |lua, ()| {
                let waiting = waiting.clone();
                async move {
                    waiting.lock().await.take().unwrap().await.unwrap();
                    let object: AnyUserData = lua.globals().get("margatroid")?;
                    drop(object.borrow_mut::<Margatroid>()?);
                    Ok("written")
                }
            })
            .unwrap(),
        )
        .unwrap();
    Margatroid::enable(&host, callbacks).unwrap();
    let result: (String, String) = lua
        .load(
            r#"
        return margatroid:await_any({
            receive = function() return margatroid:task_receive() end,
            write = function() return margatroid:task_write() end,
        })
    "#,
        )
        .eval_async()
        .await
        .unwrap();
    assert_eq!(result, ("write".into(), "written".into()));
}

#[test]
fn metadata_getters_preserve_nested_values_without_a_host_depth_setting() {
    let (lua, _) = fixture();
    lua.load("assert(margatroid.arguments.nested.value == 42); assert(margatroid.context.binding.nested.value == 'trusted')").exec().unwrap();
}
