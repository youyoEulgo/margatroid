use super::*;
use std::time::Duration;

fn setup(limits: ToolExecutionLimits) -> (Lua, ProcessRegistry) {
    let lua = Lua::new();
    let registry = install_process(&lua, std::env::current_dir().unwrap(), limits).unwrap();
    (lua, registry)
}

async fn python(lua: &Lua, code: &str) -> ProcessSession {
    let options = lua.create_table().unwrap();
    options.set("program", "/usr/bin/python3").unwrap();
    options.set("arguments", vec!["-u", "-c", code]).unwrap();
    let constructor: mlua::Function = lua.globals().get("process_open").unwrap();
    let userdata: mlua::AnyUserData = constructor.call_async(options).await.unwrap();
    userdata.borrow::<ProcessSession>().unwrap().clone()
}

async fn drain<R: AsyncRead + Unpin>(
    pipe: &AsyncMutex<Option<R>>,
    budget: &Budget,
    close: &CancellationToken,
) -> Vec<u8> {
    let mut output = Vec::new();
    loop {
        let (bytes, eof) = read_pipe(pipe, budget, close).await.unwrap();
        output.extend(bytes);
        if eof {
            return output;
        }
    }
}

#[tokio::test]
async fn real_echo_output_survives_wait_before_read() {
    let (lua, registry) = setup(ToolExecutionLimits::default());
    lua.load(
        r#"
        local p=process_open({program='/bin/echo', arguments={'hello process'}})
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
    let (lua, registry) = setup(ToolExecutionLimits::default());
    let session = python(&lua, "import sys\nn=65536\ndata=sys.stdin.buffer.read(n)\nsys.stdout.buffer.write(data)\nsys.stdout.buffer.flush()\nsys.stderr.buffer.write(bytes(range(256))*256)\nsys.stderr.buffer.flush()\nsys.exit(7)").await;
    let bytes: Vec<u8> = (0..65536).map(|n| n as u8).collect();
    let (written, stdout, stderr, status) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(
            session.write(&bytes),
            drain(
                &session.inner.stdout,
                &session.inner.stdout_budget,
                &session.inner.close
            ),
            drain(
                &session.inner.stderr,
                &session.inner.stderr_budget,
                &session.inner.close
            ),
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
    let (lua, registry) = setup(ToolExecutionLimits::default());
    tokio::time::timeout(Duration::from_secs(5), lua.load(r#"
        local p = process_open({program='/usr/bin/python3', arguments={'-u','-c',
            'import sys; b=sys.stdin.buffer.read(4); sys.stdout.buffer.write(b); sys.stderr.buffer.write(b)'}})
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
    let (lua, registry) = setup(ToolExecutionLimits::default());
    let directory = tempfile::tempdir().unwrap();
    let gate = directory.path().join("read-enabled");
    let code = format!(
        "import fcntl,os,sys,time\nfcntl.fcntl(sys.stdin.fileno(),fcntl.F_SETPIPE_SZ,4096)\nsys.stdout.buffer.write(b'ready'); sys.stdout.buffer.flush()\nwhile not os.path.exists({:?}): time.sleep(0.001)\ndata=sys.stdin.buffer.read(131072)\nsys.stdout.buffer.write(data); sys.stdout.buffer.flush()",
        gate.to_str().unwrap()
    );
    let session = python(&lua, &code).await;
    let (ready, eof) = read_pipe(
        &session.inner.stdout,
        &session.inner.stdout_budget,
        &session.inner.close,
    )
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
            drain(
                &session.inner.stdout,
                &session.inner.stdout_budget,
                &session.inner.close
            ),
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
async fn lua_await_any_concurrently_reads_stderr_waits_and_writes_one_session() {
    let (lua, registry) = setup(ToolExecutionLimits::default());
    crate::handler::install_await_any(&lua).unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), lua.load(r#"
        local p=process_open({program='/usr/bin/python3', arguments={'-u','-c',
            'import sys; b=sys.stdin.buffer.read(4); sys.stdout.buffer.write(b); sys.stderr.buffer.write(b)'}})
        local bytes=string.char(0,255,128,10)
        local operations={
            stdout=function() return p:read() end,
            stderr=function() return p:read_stderr() end,
            wait=function() return p:wait() end,
            write=function() p:write(bytes); return true end,
        }
        local got={}
        while next(operations) do
            local name, value=await_any(operations)
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
    let (lua, registry) = setup(ToolExecutionLimits::default());
    let session = python(&lua, "import time; time.sleep(60)").await;
    let bytes = vec![0; ToolExecutionLimits::default().argument_bytes];
    let (read, write, status, closed) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(
            read_pipe(
                &session.inner.stdout,
                &session.inner.stdout_budget,
                &session.inner.close
            ),
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
    let (lua, registry) = setup(ToolExecutionLimits::default());
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
    assert!(lua
        .load("process_open({program='/bin/true'})")
        .exec_async()
        .await
        .is_err());
    registry.cleanup().await.unwrap();
}

#[tokio::test]
async fn budgets_are_shared_across_sessions_and_stderr_is_separate() {
    let limits = ToolExecutionLimits {
        output_bytes: 3,
        diagnostic_bytes: 2,
        ..ToolExecutionLimits::default()
    };
    let (lua, registry) = setup(limits);
    let first = python(
        &lua,
        "import sys; sys.stdout.write('abc'); sys.stderr.write('xyz')",
    )
    .await;
    assert_eq!(
        drain(
            &first.inner.stdout,
            &first.inner.stdout_budget,
            &first.inner.close
        )
        .await,
        b"abc"
    );
    let error = read_pipe(
        &first.inner.stderr,
        &first.inner.stderr_budget,
        &first.inner.close,
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind, Kind::OutputTooLarge);
    let second = python(&lua, "print('x', end='')").await;
    let error = read_pipe(
        &second.inner.stdout,
        &second.inner.stdout_budget,
        &second.inner.close,
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind, Kind::OutputTooLarge);
    registry.cleanup().await.unwrap();
}

#[tokio::test]
async fn lua_budget_errors_preserve_tool_error_category() {
    let limits = ToolExecutionLimits {
        output_bytes: 1,
        ..ToolExecutionLimits::default()
    };
    let (lua, registry) = setup(limits);
    let error = lua
        .load(
            r#"
        local p=process_open({program='/bin/echo', arguments={'overflow'}})
        p:read()
    "#,
        )
        .exec_async()
        .await
        .unwrap_err();
    assert_eq!(
        crate::handler::lua_failure(error).kind,
        Kind::OutputTooLarge
    );
    registry.cleanup().await.unwrap();
}

#[tokio::test]
async fn program_path_and_argv_are_literal_and_cwd_is_project_root() {
    let directory = tempfile::tempdir().unwrap();
    let lua = Lua::new();
    let registry = install_process(
        &lua,
        directory.path().to_owned(),
        ToolExecutionLimits::default(),
    )
    .unwrap();
    lua.globals()
        .set("expected", directory.path().to_str().unwrap())
        .unwrap();
    lua.load(
        r#"
        local p=process_open({program='/usr/bin/python3', arguments={'-c',
            'import os,sys; print(os.getcwd()); print(sys.argv[1])', '$(echo unsafe); *'}})
        local out=''
        while true do local r=p:read(); out=out..r.data; if r.eof then break end end
        assert(out==expected..'\n$(echo unsafe); *\n')
        assert(p:wait().exit_code==0)
        assert(not pcall(process_open,{program='python3'}))
    "#,
    )
    .exec_async()
    .await
    .unwrap();
    registry.cleanup().await.unwrap();
}
