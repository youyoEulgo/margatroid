use super::*;

fn setup(root: PathBuf) -> Lua {
    let lua = Lua::new();
    let callbacks = lua.create_table().unwrap();
    install(&lua, &callbacks, root).unwrap();
    lua.globals().set("data", callbacks).unwrap();
    lua.globals().set("null", lua.null()).unwrap();
    lua
}

#[test]
fn codecs_preserve_objects_arrays_null_and_standard_scalars() {
    let lua = setup(PathBuf::new());
    lua.load(r#"
        assert(data.json_encode({}) == '[]')
        local source = '{"a":[],"n":null,"o":{},"s":"你好","v":[{},[],null]}'
        assert(data.json_encode(data.json_decode(source)) == source)
        assert(data.json_decode('null') == null)
        local yaml = data.yaml_decode('object: {}\narray: []\nnull_value: null\nlist:\n  - true\n  - 42\n')
        assert(data.json_encode(yaml.object) == '{}')
        assert(data.json_encode(yaml.array) == '[]')
        assert(yaml.null_value == null and yaml.list[1] == true and math.type(yaml.list[2]) == 'integer')
        assert(data.json_encode(data.yaml_decode(data.yaml_encode(yaml))) == data.json_encode(yaml))
        local toml = data.toml_decode('date = 1979-05-27T07:32:00Z\n[empty]\n')
        assert(toml.date == '1979-05-27T07:32:00Z')
        assert(data.json_encode(toml.empty) == '{}')
        assert(data.json_encode(data.toml_decode(data.toml_encode(toml))) == data.json_encode(toml))
        local bytes = string.char(0, 255, 128, 10)..'你好'
        assert(data.base64_decode(data.base64_encode(bytes)) == bytes)
        assert(data.base64_decode('') == '')
        for _, doc in ipairs({'0','-9223372036854775808','9223372036854775807'}) do
            assert(data.json_encode(data.json_decode(doc)) == doc)
        end
    "#).exec().unwrap();
}

#[test]
fn malformed_and_unrepresentable_data_is_rejected() {
    let lua = setup(PathBuf::new());
    lua.load(r#"
        for _, doc in ipairs({'a: [', 'a: 1\na: 2', '1: value', '!custom value', 'a: .nan', 'a: .inf', '---\na\n---\nb'}) do
            assert(not pcall(data.yaml_decode, doc), doc)
        end
        for _, doc in ipairs({'a = [', 'a = 1\na = 2', 'a = nan', 'a = +inf'}) do
            assert(not pcall(data.toml_decode, doc), doc)
        end
        assert(not pcall(data.json_decode, '{'))
        assert(not pcall(data.base64_decode, '!@#$'))
        assert(not pcall(data.base64_decode, 'Zg'))
        for _, encode in ipairs({data.json_encode, data.yaml_encode, data.toml_encode}) do
            assert(not pcall(encode, {value = 0/0}))
            assert(not pcall(encode, {value = math.huge}))
            assert(not pcall(encode, {value = function() end}))
            assert(not pcall(encode, {[true] = 'bad key'}))
            local cycle = {}; cycle.self = cycle
            assert(not pcall(encode, cycle))
        end
        assert(not pcall(data.toml_encode, {n = null}))
        assert(not pcall(data.toml_encode, {}))
        assert(not pcall(data.json_encode, {[2] = true}))
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
        data.write_file('目录/文件.bin', bytes)
        assert(data.read_file('目录/文件.bin') == bytes)
        data.write_file('empty', '')
        assert(data.read_file('empty') == '')
        local meta = data.metadata('目录/文件.bin')
        assert(meta.kind == 'file' and meta.size_bytes == #bytes)
        assert(data.metadata('目录').kind == 'directory')
        local entries = data.read_dir('.')
        assert(#entries == 2 and entries[1].name == 'empty' and entries[2].name == '目录')
        assert(entries[1].kind == 'file' and entries[2].kind == 'directory')
        assert(#data.read_dir('目录') == 1)
        assert(data.read_file(entries[1].path) == '')
        assert(data.canonicalize('目录/../empty'):sub(-6) == '/empty')
        assert(not pcall(data.read_file, 'missing'))
        assert(not pcall(data.read_file, '目录'))
        assert(not pcall(data.read_dir, 'empty'))
        assert(not pcall(data.metadata, 'missing'))
        assert(not pcall(data.canonicalize, 'missing'))
        assert(not pcall(data.write_file, 'missing/child', 'x'))
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
        assert(data.metadata('link').kind == 'symlink')
        assert(data.metadata('broken').kind == 'symlink')
        assert(data.read_file('link') == 'payload')
        assert(data.canonicalize('link') == data.canonicalize('target'))
        local entries = data.read_dir('.')
        assert(entries[1].name == 'broken' and entries[1].kind == 'symlink')
        assert(entries[2].name == 'link' and entries[2].kind == 'symlink')
        assert(not pcall(data.canonicalize, 'broken'))
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
        local decoded = data.yaml_decode(doc)
        assert(decoded.b[4][3] == 123456789)
        local bytes = string.rep(string.char(0, 255, 128), 1024)
        assert(data.base64_decode(data.base64_encode(bytes)) == bytes)
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
        data.write_file('payload', bytes)
        assert(data.read_file('payload') == bytes)
        assert(#data.read_dir('.') == 1)
        assert(data.json_decode(data.json_encode(bytes)) == bytes)
        assert(data.base64_decode(data.base64_encode(bytes)) == bytes)
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
            local decoded = data[codec..'_decode'](data[codec..'_encode'](nested))
            for i = 1, 40 do decoded = decoded.child end
            assert(decoded.value == 42)
        end
        local yaml = ''
        for i = 0, 39 do yaml = yaml .. string.rep(' ', i * 2) .. 'child:\n' end
        yaml = yaml .. string.rep(' ', 80) .. 'value: 42\n'
        local decoded = data.yaml_decode(yaml)
        for i = 1, 40 do decoded = decoded.child end
        assert(decoded.value == 42)
    "#,
    )
    .exec()
    .unwrap();
}
