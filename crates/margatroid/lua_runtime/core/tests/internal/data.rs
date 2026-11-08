use super::*;

pub(super) fn setup(root: PathBuf) -> Lua {
    let lua = Lua::new();
    let callbacks = lua.create_table().unwrap();
    install(&lua, &callbacks, root).unwrap();
    lua.globals().set("data", callbacks).unwrap();
    lua.globals().set("null", lua.null()).unwrap();
    lua
}

#[test]
fn object_conversion_accepts_plain_empty_tables_and_rejects_arrays_and_scalars() {
    let lua = setup(PathBuf::new());
    for expression in [
        "{}",
        "data.data_decode('json', '{}')",
        "data.data_decode('yaml', '{}')",
    ] {
        let value = lua
            .load(format!("return {expression}"))
            .eval::<LuaValue>()
            .unwrap();
        assert_eq!(from_lua_object(&lua, value).unwrap(), serde_json::json!({}));
    }
    let value = lua
        .load("return {message='ok', nested={items={1,2}}}")
        .eval::<LuaValue>()
        .unwrap();
    assert_eq!(
        from_lua_object(&lua, value).unwrap(),
        serde_json::json!({"message":"ok","nested":{"items":[1,2]}})
    );
    for expression in [
        "data.data_decode('json', '[]')",
        "data.data_decode('yaml', '[]')",
        "{1,2}",
        "{[1]=true, name='mixed'}",
        "null",
        "nil",
        "true",
        "42",
        "'text'",
    ] {
        let value = lua
            .load(format!("return {expression}"))
            .eval::<LuaValue>()
            .unwrap();
        assert!(from_lua_object(&lua, value).is_err(), "{expression}");
    }
    let value = lua.create_table().unwrap();
    assert_eq!(
        from_lua(&lua, LuaValue::Table(value)).unwrap(),
        serde_json::json!([])
    );
}

#[test]
fn codecs_preserve_objects_arrays_null_and_standard_scalars() {
    let lua = setup(PathBuf::new());
    lua.load(r#"
        assert(data.data_encode('json', {}) == '[]')
        local source = '{"a":[],"n":null,"o":{},"s":"你好","v":[{},[],null]}'
        assert(data.data_encode('json', data.data_decode('json', source)) == source)
        assert(data.data_decode('json', 'null') == null)
        local yaml = data.data_decode('yaml', 'object: {}\narray: []\nnull_value: null\nlist:\n  - true\n  - 42\n')
        assert(data.data_encode('json', yaml.object) == '{}')
        assert(data.data_encode('json', yaml.array) == '[]')
        assert(yaml.null_value == null and yaml.list[1] == true and math.type(yaml.list[2]) == 'integer')
        assert(data.data_encode('json', data.data_decode('yaml', data.data_encode('yaml', yaml))) == data.data_encode('json', yaml))
        local toml = data.data_decode('toml', 'date = 1979-05-27T07:32:00Z\n[empty]\n')
        assert(toml.date == '1979-05-27T07:32:00Z')
        assert(data.data_encode('json', toml.empty) == '{}')
        assert(data.data_encode('json', data.data_decode('toml', data.data_encode('toml', toml))) == data.data_encode('json', toml))
        local bytes = string.char(0, 255, 128, 10)..'你好'
        assert(data.data_decode('base64', data.data_encode('base64', bytes)) == bytes)
        assert(data.data_decode('base64', '') == '')
        for _, doc in ipairs({'0','-9223372036854775808','9223372036854775807'}) do
            assert(data.data_encode('json', data.data_decode('json', doc)) == doc)
        end
    "#).exec().unwrap();
}

#[test]
fn malformed_and_unrepresentable_data_is_rejected() {
    let lua = setup(PathBuf::new());
    lua.load(r#"
        for _, doc in ipairs({'a: [', 'a: 1\na: 2', '1: value', '!custom value', 'a: .nan', 'a: .inf', '---\na\n---\nb'}) do
            assert(not pcall(function(v) return data.data_decode('yaml', v) end, doc), doc)
        end
        for _, doc in ipairs({'a = [', 'a = 1\na = 2', 'a = nan', 'a = +inf'}) do
            assert(not pcall(function(v) return data.data_decode('toml', v) end, doc), doc)
        end
        assert(not pcall(function(v) return data.data_decode('json', v) end, '{'))
        assert(not pcall(function(v) return data.data_decode('base64', v) end, '!@#$'))
        assert(not pcall(function(v) return data.data_decode('base64', v) end, 'Zg'))
        for _, encode in ipairs({function(v) return data.data_encode('json', v) end, function(v) return data.data_encode('yaml', v) end, function(v) return data.data_encode('toml', v) end}) do
            assert(not pcall(encode, {value = 0/0}))
            assert(not pcall(encode, {value = math.huge}))
            assert(not pcall(encode, {value = function() end}))
            assert(not pcall(encode, {[true] = 'bad key'}))
            local cycle = {}; cycle.self = cycle
            assert(not pcall(encode, cycle))
        end
        assert(not pcall(function(v) return data.data_encode('toml', v) end, {n = null}))
        assert(not pcall(function(v) return data.data_encode('toml', v) end, {}))
        assert(not pcall(function(v) return data.data_encode('json', v) end, {[2] = true}))
    "#).exec().unwrap();
}

#[tokio::test]
async fn filesystem_round_trip_binary_unicode_directory_and_errors() {
    let root = tempfile::tempdir().unwrap();
    tokio::fs::create_dir(root.path().join("目录"))
        .await
        .unwrap();
    let lua = setup(root.path().to_owned());
    lua.load(
        r#"
        local bytes = string.char(0, 255, 128, 10)..'你好'
        data.files_write('目录/文件.bin', bytes)
        assert(data.files_read('目录/文件.bin') == bytes)
        data.files_write('empty', '')
        assert(data.files_read('empty') == '')
        local meta = data.files_info('目录/文件.bin')
        assert(meta.kind == 'file' and meta.size == #bytes)
        assert(data.files_info('目录').kind == 'directory')
        local entries = data.files_list('.')
        assert(#entries == 2 and entries[1].name == 'empty' and entries[2].name == '目录')
        assert(entries[1].kind == 'file' and entries[2].kind == 'directory')
        assert(#data.files_list('目录') == 1)
        assert(data.files_read(entries[1].path) == '')
        assert(data.files_info('目录/../empty').resolved_path:sub(-6) == '/empty')
        assert(not pcall(data.files_read, 'missing'))
        assert(not pcall(data.files_read, '目录'))
        assert(not pcall(data.files_list, 'empty'))
        assert(not pcall(data.files_info, 'missing'))
        assert(not pcall(data.files_info, 'missing'))
        assert(not pcall(data.files_write, 'missing/child', 'x'))
    "#,
    )
    .exec_async()
    .await
    .unwrap();
    assert!(!root.path().join("missing").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn filesystem_symlinks_are_reported_without_following_metadata() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("target"), b"payload").unwrap();
    std::os::unix::fs::symlink("target", root.path().join("link")).unwrap();
    std::os::unix::fs::symlink("missing", root.path().join("broken")).unwrap();
    let lua = setup(root.path().to_owned());
    lua.load(
        r#"
        assert(data.files_info('link').kind == 'symlink')
        assert(data.files_info('broken').kind == 'symlink')
        assert(data.files_read('link') == 'payload')
        assert(data.files_info('link').resolved_path == data.files_info('target').resolved_path)
        local entries = data.files_list('.')
        assert(entries[1].name == 'broken' and entries[1].kind == 'symlink')
        assert(entries[2].name == 'link' and entries[2].kind == 'symlink')
        assert(data.files_info('broken').resolved_path == nil)
    "#,
    )
    .exec_async()
    .await
    .unwrap();
}

#[test]
fn yaml_aliases_and_binary_base64_are_not_subject_to_host_byte_limits() {
    let lua = setup(PathBuf::new());
    lua.load(
        r#"
        local doc = 'a: &a [123456789, 123456789, 123456789]\nb: [*a, *a, *a, *a]'
        local decoded = data.data_decode('yaml', doc)
        assert(decoded.b[4][3] == 123456789)
        local bytes = string.rep(string.char(0, 255, 128), 1024)
        assert(data.data_decode('base64', data.data_encode('base64', bytes)) == bytes)
    "#,
    )
    .exec()
    .unwrap();
}

#[tokio::test]
async fn filesystem_and_codecs_accept_values_above_old_tiny_byte_limits() {
    let root = tempfile::tempdir().unwrap();
    let lua = setup(root.path().to_owned());
    lua.load(
        r#"
        local bytes = string.rep('你好', 1024)
        data.files_write('payload', bytes)
        assert(data.files_read('payload') == bytes)
        assert(#data.files_list('.') == 1)
        assert(data.data_decode('json', data.data_encode('json', bytes)) == bytes)
        assert(data.data_decode('base64', data.data_encode('base64', bytes)) == bytes)
    "#,
    )
    .exec_async()
    .await
    .unwrap();
}

#[test]
fn codecs_accept_nested_documents_without_a_host_depth_setting() {
    let lua = setup(PathBuf::new());
    lua.load(
        r#"
        local nested, cursor = {}, nil
        cursor = nested
        for i = 1, 40 do cursor.child = {}; cursor = cursor.child end
        cursor.value = 42
        for _, codec in ipairs({'json', 'yaml', 'toml'}) do
            local decoded = data.data_decode(codec, data.data_encode(codec, nested))
            for i = 1, 40 do decoded = decoded.child end
            assert(decoded.value == 42)
        end
        local yaml = ''
        for i = 0, 39 do yaml = yaml .. string.rep(' ', i * 2) .. 'child:\n' end
        yaml = yaml .. string.rep(' ', 80) .. 'value: 42\n'
        local decoded = data.data_decode('yaml', yaml)
        for i = 1, 40 do decoded = decoded.child end
        assert(decoded.value == 42)
    "#,
    )
    .exec()
    .unwrap();
}
