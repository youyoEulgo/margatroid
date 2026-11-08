use lua_runtime_core::host::{
    self, HostDefinition, HostKind, LuaFailureClass, LuaFailureDiagnostic, Margatroid,
};
use mlua::{AnyUserData, Lua};
use serde_json::json;

fn host(kind: HostKind) -> (Lua, AnyUserData) {
    let lua = Lua::new();
    let object = Margatroid::install(
        &lua,
        HostDefinition {
            kind,
            context: json!({"kind":"spoof", "identity":"trusted", "object":{},"array":[],"null":null,"arguments":"spoof"}),
            arguments: json!({"object":{}, "array":[], "nested":{"value":42},"null":null}),
        },
    )
    .unwrap();
    (lua, object)
}

#[tokio::test]
async fn configure_is_metadata_only_and_identity_fields_are_readonly_copies() {
    let (lua, _) = host(HostKind::Tool);
    lua.load(r#"
        assert(type(margatroid)=='userdata')
        assert(margatroid.context.kind=='tool')
        local loaded_context=margatroid.context; loaded_context.kind='driver'; loaded_context.identity='tampered'
        local arguments=margatroid.context.arguments; arguments.nested.value=0
        assert(margatroid.context.kind=='tool' and margatroid.context.identity=='trusted')
        assert(margatroid.context.null == margatroid.data.null)
        assert(margatroid.context.arguments.null == margatroid.data.null)
        assert(margatroid.context.arguments.nested.value==42)
        for _,field in ipairs({'context','files','data','text','process','driver','tools','task'}) do
            assert(not pcall(function() margatroid[field]={} end))
        end
        assert(not pcall(function() margatroid() end))
        assert(not pcall(function() margatroid:task_write('old alias') end))
        assert(task_write==nil and process_open==nil and args==nil and context==nil)
        assert(margatroid.data.null ~= nil)
        assert(not pcall(function() margatroid.data.null = 'changed' end))
        for _, old in ipairs({'version','arguments','null','tool','process_open','await_any',
            'read_file','write_file','read_dir','metadata','canonicalize','json_encode',
            'json_decode','yaml_encode','yaml_decode','toml_encode','toml_decode',
            'base64_encode','base64_decode'}) do
            local ok, value = pcall(function() return margatroid[old] end)
            assert(not ok or value == nil, old)
        end
        for _, name in ipairs({'files','data','text','process','driver','tools','task'}) do
            assert(type(margatroid[name]) == 'userdata')
            assert(margatroid[name] == margatroid[name])
            assert(not pcall(function() margatroid[name].extra = true end))
        end
    "#).exec().unwrap();
    for expression in [
        "margatroid:log('info','x')",
        "margatroid.data:encode('json', {})",
        "margatroid.data:decode('json', '{}')",
        "margatroid.files:read('x')",
        "margatroid.text:search('x', 'x')",
        "margatroid.tools:register({})",
        "margatroid.process:start('/bin/true', {})",
        "margatroid.task:append_output('x')",
    ] {
        let error = lua.load(expression).exec_async().await.unwrap_err();
        assert!(
            error.to_string().contains("unavailable during configure"),
            "{error}"
        );
    }
}

#[tokio::test]
async fn enable_freezes_callbacks_and_retains_only_current_identity() {
    for kind in [HostKind::Driver, HostKind::Tool] {
        let (lua, object) = host(kind);
        let callbacks = lua.create_table().unwrap();
        host::data::install(&lua, &callbacks, std::path::PathBuf::new()).unwrap();
        callbacks
            .set(
                "driver_mcl",
                lua.create_async_function(|_, (): ()| async { Ok("driver") })
                    .unwrap(),
            )
            .unwrap();
        callbacks
            .set(
                "task_read_output",
                lua.create_async_function(|_, (): ()| async { Ok("tool") })
                    .unwrap(),
            )
            .unwrap();
        callbacks
            .set(
                "log",
                lua.create_async_function(|_, (_level, _text): (String, String)| async {
                    tokio::task::yield_now().await;
                    Ok(())
                })
                .unwrap(),
            )
            .unwrap();
        Margatroid::enable(&object, callbacks.clone()).unwrap();
        callbacks
            .set(
                "task_read_output",
                lua.create_async_function(|_, (): ()| async { Ok("tampered") })
                    .unwrap(),
            )
            .unwrap();
        callbacks
            .set(
                "driver_mcl",
                lua.create_async_function(|_, (): ()| async { Ok("tampered") })
                    .unwrap(),
            )
            .unwrap();
        lua.load(
            r#"
            assert(margatroid.data:encode('json', margatroid.context.object)=='{}')
            assert(margatroid.data:encode('json', margatroid.context.array)=='[]')
            assert(margatroid.data:encode('json', margatroid.context.arguments.object)=='{}')
            assert(margatroid.data:encode('json', margatroid.context.arguments.array)=='[]')
            margatroid:log('info','hello')
            if margatroid.context.kind=='driver' then
                assert(margatroid.driver:mcl()=='driver')
                assert(not pcall(function() margatroid.task:read_output() end))
            else
                assert(margatroid.task:read_output()=='tool')
                assert(not pcall(function() margatroid.driver:mcl() end))
            end
        "#,
        )
        .exec_async()
        .await
        .unwrap();
        assert!(Margatroid::enable(&object, callbacks).is_err());
    }
}

#[tokio::test]
async fn every_group_dispatches_plain_callbacks_and_rejects_wrong_capabilities() {
    let groups: &[(&str, &[&str])] = &[
        (
            "files",
            &[
                "read",
                "write",
                "list",
                "info",
                "create_directory",
                "matches",
            ],
        ),
        ("data", &["encode", "decode"]),
        ("text", &["search"]),
        ("process", &["start"]),
        ("driver", &["mcl", "wait"]),
        ("tools", &["register"]),
        (
            "task",
            &[
                "return_result",
                "finish",
                "append_output",
                "replace_output",
                "read_output",
                "send",
                "receive",
            ],
        ),
    ];
    assert_eq!(groups.len(), 7);
    assert_eq!(
        groups
            .iter()
            .map(|(_, methods)| methods.len())
            .sum::<usize>(),
        20
    );
    for kind in [HostKind::Driver, HostKind::Tool] {
        let (lua, object) = host(kind);
        let callbacks = lua.create_table().unwrap();
        for &(group, names) in groups {
            for &name in names {
                let key = format!("{group}_{name}");
                let expected = key.clone();
                callbacks
                    .set(
                        key,
                        lua.create_function(move |_, (text, number): (String, i64)| {
                            assert_eq!(text, "literal");
                            assert_eq!(number, 42);
                            Ok(expected.clone())
                        })
                        .unwrap(),
                    )
                    .unwrap();
            }
        }
        for &(group, names) in groups {
            let allowed = match group {
                "driver" => kind == HostKind::Driver,
                "tools" | "task" => kind == HostKind::Tool,
                _ => true,
            };
            for &name in names {
                let error = lua
                    .load(format!("margatroid.{group}:{name}('literal', 42)"))
                    .exec_async()
                    .await
                    .unwrap_err();
                let cause = error
                    .chain()
                    .find_map(|cause| cause.downcast_ref::<host::SharedHostError>())
                    .unwrap();
                assert_eq!(
                    cause.kind,
                    if allowed {
                        host::SharedHostErrorKind::Unavailable
                    } else {
                        host::SharedHostErrorKind::InvalidExecutionContext
                    }
                );
            }
        }
        // The callback selection must also survive clearing the source table.
        Margatroid::enable(&object, callbacks.clone()).unwrap();
        callbacks.clear().unwrap();
        for &(group, names) in groups {
            let allowed = match group {
                "driver" => kind == HostKind::Driver,
                "tools" | "task" => kind == HostKind::Tool,
                _ => true,
            };
            for &name in names {
                let result = lua
                    .load(format!("return margatroid.{group}:{name}('literal', 42)"))
                    .eval_async::<String>()
                    .await;
                if allowed {
                    assert_eq!(result.unwrap(), format!("{group}_{name}"));
                } else {
                    let error = result.unwrap_err();
                    let cause = error
                        .chain()
                        .find_map(|cause| cause.downcast_ref::<host::SharedHostError>())
                        .unwrap();
                    assert_eq!(
                        cause.kind,
                        host::SharedHostErrorKind::InvalidExecutionContext
                    );
                }
            }
        }
        for expression in [
            "margatroid.files:encode()",
            "margatroid.data:read()",
            "margatroid.text:start()",
        ] {
            let error = lua.load(expression).exec_async().await.unwrap_err();
            let cause = error
                .chain()
                .find_map(|cause| cause.downcast_ref::<host::SharedHostError>())
                .unwrap();
            assert_eq!(
                cause.kind,
                host::SharedHostErrorKind::InvalidExecutionContext
            );
        }
    }
}

#[derive(Debug)]
struct DomainError;
impl std::fmt::Display for DomainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DomainKind: actual business message")
    }
}
impl std::error::Error for DomainError {}

#[test]
fn diagnostics_preserve_actual_external_class_and_message_without_wrapper() {
    let lua = Lua::new();
    lua.globals()
        .set(
            "fail",
            lua.create_function(|_, (): ()| -> mlua::Result<()> {
                Err(mlua::Error::external(DomainError))
            })
            .unwrap(),
        )
        .unwrap();
    let error = lua
        .load("fail()")
        .set_name("diagnostic.lua")
        .exec()
        .unwrap_err();
    let diagnostic = LuaFailureDiagnostic::capture_with(&error, |cause| {
        cause
            .downcast_ref::<DomainError>()
            .map(|_| "actual business message".into())
    });
    assert_eq!(diagnostic.class, LuaFailureClass::External);
    assert_eq!(diagnostic.message, "actual business message");
    assert!(diagnostic.traceback.unwrap().contains("diagnostic.lua"));
    let message = "business prefix:42: retain\nstack traceback: authored details";
    let diagnostic = LuaFailureDiagnostic::capture(&mlua::Error::external(message));
    assert_eq!(diagnostic.message, message);
}

#[tokio::test]
async fn concurrent_namespace_dispatch_releases_userdata_borrows_before_await() {
    let (lua, object) = host(HostKind::Tool);
    let callbacks = lua.create_table().unwrap();
    callbacks
        .set("wait_any", host::install_wait_any(&lua).unwrap())
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
            "task_append_output",
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
    Margatroid::enable(&object, callbacks).unwrap();
    let result: (String, String) = lua
        .load(
            r#"return margatroid:wait_any({
        receive=function() return margatroid.task:receive() end,
        write=function() return margatroid.task:append_output() end,
    })"#,
        )
        .eval_async()
        .await
        .unwrap();
    assert_eq!(result, ("write".into(), "written".into()));
}

#[test]
fn diagnostic_file_origins_preserve_full_raw_trace_and_clean_message() {
    let lua = Lua::new();
    for origin in [
        "@/some dir/resource/main.lua",
        "@C:/some dir/resource/main.lua",
        "@protocol-fixture.lua",
        "@Directory file.lua",
    ] {
        let error = lua
            .load("error('specific failure')")
            .set_name(origin)
            .exec()
            .unwrap_err();
        let raw = error.to_string();
        let captured = LuaFailureDiagnostic::capture(&error);
        assert_eq!(captured.message, "specific failure");
        let location = format!("{}:1", origin.trim_start_matches('@'));
        assert!(raw.contains(&location));
        assert!(captured.traceback.unwrap().contains(&location));
        let external = LuaFailureDiagnostic::capture(&mlua::Error::external(format!(
            "{location}: business message"
        )));
        assert_eq!(external.message, format!("{location}: business message"));
        let syntax = lua.load("local =").set_name(origin).exec().unwrap_err();
        let captured = LuaFailureDiagnostic::capture(&syntax);
        assert_eq!(captured.class, LuaFailureClass::Syntax);
        assert!(!captured.message.contains(origin.trim_start_matches('@')));
    }
}

#[test]
fn diagnostics_split_real_traceback_and_strip_only_anchored_lua_location() {
    let lua = Lua::new();
    let error = lua.load("error('specific failure')").exec().unwrap_err();
    let captured = LuaFailureDiagnostic::capture(&error);
    assert_eq!(captured.class, LuaFailureClass::Runtime);
    assert_eq!(captured.message, "specific failure");
    assert!(captured.traceback.unwrap().contains("stack traceback:"));
    let captured = LuaFailureDiagnostic::capture(&mlua::Error::RuntimeError(
        "business report:12: preserve".into(),
    ));
    assert_eq!(captured.message, "business report:12: preserve");
}
