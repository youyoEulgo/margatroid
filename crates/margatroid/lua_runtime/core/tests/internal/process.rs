use super::*;
use std::time::Duration;

fn setup() -> (Lua, ProcessRegistry) {
    setup_at(std::env::current_dir().unwrap())
}

fn setup_at(root: std::path::PathBuf) -> (Lua, ProcessRegistry) {
    let lua = Lua::new();
    let (registry, process_start) = install_process(&lua, root.clone()).unwrap();
    let object = crate::host::Margatroid::install(
        &lua,
        crate::host::HostDefinition {
            kind: crate::host::HostKind::Tool,
            context: serde_json::json!({"project_root": root}),
            arguments: serde_json::json!({}),
        },
    )
    .unwrap();
    let callbacks = lua.create_table().unwrap();
    callbacks.set("process_start", process_start).unwrap();
    callbacks
        .set("wait_any", crate::host::install_wait_any(&lua).unwrap())
        .unwrap();
    crate::host::Margatroid::enable(&object, callbacks).unwrap();
    (lua, registry)
}

async fn python(lua: &Lua, code: &str) -> ProcessSession {
    let arguments = vec!["-u", "-c", code];
    let constructor: mlua::Function = lua
        .load("return function(program, args) return margatroid.process:start(program, args) end")
        .eval()
        .unwrap();
    let userdata: mlua::AnyUserData = constructor
        .call_async(("/usr/bin/python3", arguments))
        .await
        .unwrap();
    userdata.borrow::<ProcessSession>().unwrap().clone()
}

async fn drain<R: AsyncRead + Unpin>(
    pipe: &AsyncMutex<Option<R>>,
    close: &CancellationToken,
) -> Vec<u8> {
    let mut output = Vec::new();
    loop {
        let (bytes, eof) = read_pipe(pipe, close).await.unwrap();
        output.extend(bytes);
        if eof {
            return output;
        }
    }
}

#[tokio::test]
async fn constructor_requires_program_and_argument_vector() {
    let (lua, registry) = setup();
    for expression in [
        "margatroid.process:start({program='/bin/true', arguments={}})",
        "margatroid.process:start('/bin/true')",
        "margatroid.process:start('', {})",
        "margatroid.process:start('/bin/true' .. string.char(0), {})",
        "margatroid.process:start('/bin/true', {string.char(0)})",
    ] {
        assert!(
            lua.load(expression).exec_async().await.is_err(),
            "{expression}"
        );
    }
    registry.cleanup().await.unwrap();
}

#[tokio::test]
async fn real_echo_output_survives_wait_before_read() {
    let (lua, registry) = setup();
    lua.load(
        r#"
        local p=margatroid.process:start('/bin/echo', {'hello process'})
        assert(p:wait().exit_code==0)
        local out=''
        while true do local r=p:read(); out=out..r.data; if r.eof then break end end
        assert(out=='hello process\n')
        assert(p:read_stderr().eof)
        p:close()
    "#,
    )
    .exec_async()
    .await
    .unwrap();
    registry.cleanup().await.unwrap();
}

#[tokio::test]
async fn binary_echo_and_stderr_are_independent_and_wait_is_concurrent() {
    let (lua, registry) = setup();
    let session = python(&lua, "import sys\nn=65536\ndata=sys.stdin.buffer.read(n)\nsys.stdout.buffer.write(data)\nsys.stdout.buffer.flush()\nsys.stderr.buffer.write(bytes(range(256))*256)\nsys.stderr.buffer.flush()\nsys.exit(7)").await;
    let bytes: Vec<u8> = (0..65536).map(|n| n as u8).collect();
    let (written, stdout, stderr, status) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(
            session.write(&bytes),
            drain(&session.inner.stdout, &session.inner.close),
            drain(&session.inner.stderr, &session.inner.close),
            session.wait(),
        )
    })
    .await
    .unwrap();
    written.unwrap();
    assert_eq!(stdout, bytes);
    assert_eq!(stderr, bytes);
    assert_eq!(status.unwrap().code(), Some(7));
    assert_eq!(session.wait().await.unwrap().code(), Some(7));
    registry.cleanup().await.unwrap();
}

#[tokio::test]
async fn lua_userdata_preserves_invalid_utf8_nuls_eof_and_status() {
    let (lua, registry) = setup();
    tokio::time::timeout(Duration::from_secs(5), lua.load(r#"
        local p = margatroid.process:start('/usr/bin/python3', {'-u','-c',
            'import sys; b=sys.stdin.buffer.read(4); sys.stdout.buffer.write(b); sys.stderr.buffer.write(b)'})
        local expected = string.char(0,255,128,10)
        p:write(expected)
        local out, err = '', ''
        while true do local r=p:read(); out=out..r.data; if r.eof then break end end
        while true do local r=p:read_stderr(); err=err..r.data; if r.eof then break end end
        assert(out==expected and err==expected)
        local status=p:wait()
        assert(status.exit_code==0 and status.signal==nil)
        assert(p:read().eof and p:read_stderr().eof)
        p:close(); p:close()
    "#).exec_async()).await.unwrap().unwrap();
    registry.cleanup().await.unwrap();
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn cancelled_partial_write_resumes_same_frame_and_rejects_different_bytes() {
    let (lua, registry) = setup();
    let directory = tempfile::tempdir().unwrap();
    let gate = directory.path().join("read-enabled");
    let code = format!(
        "import fcntl,os,sys,time\nfcntl.fcntl(sys.stdin.fileno(),fcntl.F_SETPIPE_SZ,4096)\nsys.stdout.buffer.write(b'ready'); sys.stdout.buffer.flush()\nwhile not os.path.exists({:?}): time.sleep(0.001)\ndata=sys.stdin.buffer.read(131072)\nsys.stdout.buffer.write(data); sys.stdout.buffer.flush()",
        gate.to_str().unwrap()
    );
    let session = python(&lua, &code).await;
    let (ready, eof) = read_pipe(&session.inner.stdout, &session.inner.close)
        .await
        .unwrap();
    assert_eq!(ready, b"ready");
    assert!(!eof);
    let bytes: Vec<u8> = (0..65536u32)
        .map(|n| ((n * 73) ^ (n >> 7) ^ (n >> 11)) as u8)
        .collect();
    assert!(
        tokio::time::timeout(Duration::from_millis(20), session.write(&bytes))
            .await
            .is_err()
    );
    let offset = {
        let input = session.inner.stdin.lock().await;
        let frame = input.pending.as_ref().unwrap();
        assert_eq!(frame.bytes, bytes);
        assert!(frame.offset > 0 && frame.offset < bytes.len());
        frame.offset
    };
    let error = session.write(b"a different frame").await.unwrap_err();
    assert_eq!(error.kind, Kind::InvalidRequest);
    assert_eq!(
        session
            .inner
            .stdin
            .lock()
            .await
            .pending
            .as_ref()
            .unwrap()
            .offset,
        offset
    );
    std::fs::write(gate, b"ready").unwrap();
    let (written, output, status) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(
            async {
                session.write(&bytes).await?;
                assert!(session.inner.stdin.lock().await.pending.is_none());
                // A new, identical complete call is a second frame, not a retry.
                session.write(&bytes).await
            },
            drain(&session.inner.stdout, &session.inner.close),
            session.wait(),
        )
    })
    .await
    .unwrap();
    written.unwrap();
    assert_eq!(output, [bytes.as_slice(), bytes.as_slice()].concat());
    assert_eq!(status.unwrap().code(), Some(0));
    assert!(session.inner.stdin.lock().await.pending.is_none());
    registry.cleanup().await.unwrap();
}

#[tokio::test]
async fn lua_wait_any_concurrently_reads_stderr_waits_and_writes_one_session() {
    let (lua, registry) = setup();
    let result = tokio::time::timeout(Duration::from_secs(5), lua.load(r#"
        local p=margatroid.process:start('/usr/bin/python3', {'-u','-c',
            'import sys; b=sys.stdin.buffer.read(4); sys.stdout.buffer.write(b); sys.stderr.buffer.write(b)'})
        local bytes=string.char(0,255,128,10)
        local operations={
            stdout=function() return p:read() end,
            stderr=function() return p:read_stderr() end,
            wait=function() return p:wait() end,
            write=function() p:write(bytes); return true end,
        }
        local got={}
        while next(operations) do
            local name, value=margatroid:wait_any(operations)
            got[name]=value
            operations[name]=nil
        end
        assert(got.stdout.data==bytes and got.stderr.data==bytes)
        assert(got.wait.exit_code==0 and got.write)
        p:close()
    "#).exec_async()).await;
    registry.cleanup().await.unwrap();
    result.unwrap().unwrap();
}

#[tokio::test]
async fn close_interrupts_concurrent_wait_read_and_backpressured_write() {
    let (lua, registry) = setup();
    let session = python(&lua, "import time; time.sleep(60)").await;
    let bytes = vec![0; 1024 * 1024];
    let (read, write, status, closed) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(
            read_pipe(&session.inner.stdout, &session.inner.close),
            session.write(&bytes),
            session.wait(),
            async {
                tokio::task::yield_now().await;
                session.close().await
            },
        )
    })
    .await
    .unwrap();
    assert_eq!(read.unwrap_err().kind, Kind::Closed);
    assert_eq!(write.unwrap_err().kind, Kind::Closed);
    closed.unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.unwrap().signal(), Some(9));
    }
    registry.cleanup().await.unwrap();
}

#[tokio::test]
async fn worker_cleanup_after_cancelled_lua_reaps_every_session_and_closes_admission() {
    let (lua, registry) = setup();
    let first = python(&lua, "import time; time.sleep(60)").await;
    let second = python(&lua, "import time; time.sleep(60)").await;
    lua.globals()
        .set("p", lua.create_userdata(first.clone()).unwrap())
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(20), lua.load("p:read()").exec_async())
            .await
            .is_err()
    );
    tokio::time::timeout(Duration::from_secs(5), registry.cleanup())
        .await
        .unwrap()
        .unwrap();
    assert!(first.wait().await.unwrap().code().is_none());
    assert!(second.wait().await.unwrap().code().is_none());
    let input = first.inner.stdin.lock().await;
    assert!(input.stdin.is_none());
    assert!(input.pending.is_none());
    drop(input);
    assert!(second.inner.stdout.lock().await.is_none());
    assert!(
        lua.load("margatroid.process:start('/bin/true', {})")
            .exec_async()
            .await
            .is_err()
    );
    registry.cleanup().await.unwrap();
}

#[tokio::test]
async fn output_is_preserved_across_sessions_and_delayed_reads() {
    let (lua, registry) = setup();
    let first = python(
        &lua,
        "import sys; sys.stdout.write('a'*256); sys.stderr.write('b'*256)",
    )
    .await;
    let second = python(&lua, "print('second', end='')").await;
    assert_eq!(first.wait().await.unwrap().code(), Some(0));
    assert_eq!(second.wait().await.unwrap().code(), Some(0));
    assert_eq!(
        drain(&first.inner.stdout, &first.inner.close).await,
        vec![b'a'; 256]
    );
    assert_eq!(
        drain(&first.inner.stderr, &first.inner.close).await,
        vec![b'b'; 256]
    );
    assert_eq!(
        drain(&second.inner.stdout, &second.inner.close).await,
        b"second"
    );
    registry.cleanup().await.unwrap();
}

#[tokio::test]
async fn program_path_and_argv_are_literal_and_cwd_is_project_root() {
    let directory = tempfile::tempdir().unwrap();
    let (lua, registry) = setup_at(directory.path().to_owned());
    lua.globals()
        .set("expected", directory.path().to_str().unwrap())
        .unwrap();
    lua.load(
        r#"
        local p=margatroid.process:start('/usr/bin/python3', {'-c',
            'import os,sys; print(os.getcwd()); print(sys.argv[1])', '$(echo unsafe); *'})
        local out=''
        while true do local r=p:read(); out=out..r.data; if r.eof then break end end
        assert(out==expected..'\n$(echo unsafe); *\n')
        assert(p:wait().exit_code==0)
        assert(not pcall(function() margatroid.process:start('python3', {}) end))
    "#,
    )
    .exec_async()
    .await
    .unwrap();
    registry.cleanup().await.unwrap();
}
