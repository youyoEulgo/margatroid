use super::tests::setup;
use std::path::PathBuf;

#[test]
fn glob_contract_and_regex_byte_offsets() {
    let lua = setup(PathBuf::new());
    lua.load(
        r#"
        local m = data.files_matches
        assert(m('src/deep/main.rs', '*.rs'))
        assert(not m('src/deep/main.rs', 'src/*.rs'))
        assert(m('src/deep/main.rs', 'src/**/*.rs'))
        assert(m('main.rs', '**/*.rs'))
        assert(m('src/lib.rs', '{src,tests}/[!a]*.rs'))
        assert(not m('src/abc.rs', '{src,tests}/[!a]*.rs'))
        assert(m('深/文件.rs', '*.rs'))
        assert(m('deep/a*b.rs', 'a\\*b.rs'))
        assert(m('deep/!literal', '!literal'))
        assert(not pcall(m, 'a', '['))
        local s = data.text_search
        local result = s('你好|x+', 'a你好xx')
        assert(#result == 2 and result[1].start == 1 and result[1]['end'] == 7)
        assert(result[2].start == 7 and result[2]['end'] == 9)
        assert(#s('^x$', 'x\nx') == 0)
        assert(#s('(?m)^x$', 'x\nx') == 2)
        assert(#s('a+', 'aaaa') == 1)
        local zero = s('', 'ab')
        assert(#zero == 3 and zero[3].start == 2 and zero[3]['end'] == 2)
        assert(#s('.', string.char(255)) == 0)
        assert(#s('(?-u:.)', string.char(255)) == 1)
        assert(not pcall(s, '[', 'value'))
        assert(data.json_encode == nil and data.read_file == nil and data.canonicalize == nil)
        for _, format in ipairs({'JSON', 'Yaml', 'TOML', '', 'binary'}) do
            assert(not pcall(data.data_encode, format, 'x'))
            assert(not pcall(data.data_decode, format, 'x'))
        end
        assert(not pcall(data.data_encode, 'base64', 42))
    "#,
    )
    .exec()
    .unwrap();
}

#[tokio::test]
async fn explicit_directory_creation_and_complete_file_information() {
    let root = tempfile::tempdir().unwrap();
    let lua = setup(root.path().to_owned());
    lua.load(r#"
        assert(not pcall(data.files_create_directory, 'missing/child', false))
        data.files_create_directory('nested/child', true)
        data.files_create_directory('nested', false)
        data.files_create_directory('nested/child', true)
        data.files_write('payload', 'hello')
        assert(not pcall(data.files_create_directory, 'payload', false))
        assert(not pcall(data.files_create_directory, 'payload', true))
        assert(not pcall(data.files_write, 'absent/child', 'x'))
        local info = data.files_info('payload')
        assert(info.kind == 'file' and info.size == 5)
        assert(info.path:sub(-8) == '/payload' and info.resolved_path:sub(-8) == '/payload')
        assert(math.type(info.modified_at_ns) == 'integer' and info.modified_at_ns > 1000000000000000000)
        local entries = data.files_list('.')
        assert(#entries == 2 and entries[1].name == 'nested' and entries[2].name == 'payload')
        assert(entries[2].size == 5 and entries[2].modified_at_ns == info.modified_at_ns)
        assert(entries[2].resolved_path == info.resolved_path)
    "#).exec_async().await.unwrap();
    assert!(!root.path().join("missing").exists());
    assert!(!root.path().join("absent").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn raw_paths_symlink_metadata_and_nanosecond_precision() {
    use std::os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, symlink},
    };
    let root = tempfile::tempdir().unwrap();
    let name = std::ffi::OsStr::from_bytes(b"raw\xff.rs");
    let path = root.path().join(name);
    std::fs::write(&path, b"\0\xffpayload").unwrap();
    symlink(name, root.path().join("link")).unwrap();
    symlink("missing", root.path().join("broken")).unwrap();
    let lua = setup(root.path().to_owned());
    lua.globals()
        .set("raw", lua.create_string(name.as_bytes()).unwrap())
        .unwrap();
    let metadata = std::fs::symlink_metadata(&path).unwrap();
    let expected_ns = metadata.mtime() * 1_000_000_000 + metadata.mtime_nsec();
    lua.globals().set("expected_ns", expected_ns).unwrap();
    lua.load(
        r#"
        assert(data.files_read(raw) == string.char(0,255)..'payload')
        local info = data.files_info(raw)
        assert(info.path:sub(-#raw) == raw and info.resolved_path:sub(-#raw) == raw)
        assert(info.modified_at_ns == expected_ns and math.type(info.modified_at_ns) == 'integer')
        local link = data.files_info('link')
        assert(link.kind == 'symlink' and link.size == #raw)
        assert(link.resolved_path == info.resolved_path)
        assert(data.files_info(link.resolved_path).kind == 'file')
        local broken = data.files_info('broken')
        assert(broken.kind == 'symlink' and broken.size == 7 and broken.resolved_path == nil)
        assert(not pcall(data.files_info, 'missing'))
        local entries = data.files_list('.')
        assert(#entries == 3 and entries[3].name == raw and entries[3].path:sub(-#raw) == raw)
        assert(entries[3].resolved_path == info.resolved_path)
        assert(data.files_matches(raw, '*.rs'))
        data.files_write(raw, string.char(255,0))
        assert(data.files_read(raw) == string.char(255,0))
    "#,
    )
    .exec_async()
    .await
    .unwrap();
}
