#![cfg(target_os = "linux")]
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin};
use core_plugin::App;
use serde_json::json;
use std::{
    io::{Read, Write},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::oneshot;
use tool_core::{ToolExecuteRequest, ToolTasks};
use tool_plugin::ToolPlugin;

#[test]
fn real_tool_worker_http_posts_binary_and_preserves_redirect_response() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/request", listener.local_addr().unwrap());
    let (tx, rx) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut header = Vec::new();
        let mut one = [0];
        while !header.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut one).unwrap();
            header.push(one[0]);
        }
        let header = String::from_utf8(header).unwrap();
        let length = header
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .map(|value| value.trim().parse::<usize>().unwrap())
            })
            .unwrap();
        let mut body = vec![0; length];
        stream.read_exact(&mut body).unwrap();
        tx.send((header, body)).unwrap();
        stream.write_all(b"HTTP/1.1 302 Found\r\nContent-Length: 4\r\nLocation: http://127.0.0.1:1/should-not-follow\r\nX-Test: a\r\nX-Test: b\r\nConnection: close\r\n\r\n\x00\xffok").unwrap();
    });
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("tool.toml"),
        "schema_version=1\ndescription='HTTP integration'\n",
    )
    .unwrap();
    std::fs::write(
        root.path().join("input.schema.json"),
        "{\"type\":\"object\"}",
    )
    .unwrap();
    std::fs::write(root.path().join("main.lua"),r#"
        function configure() return {timeout_ms=10000} end
        function execute(arguments)
            assert(not pcall(function() margatroid.http={} end))
            assert(not pcall(function() margatroid.http.request=function()end end))
            local response=margatroid.http:request({url=arguments.url,method='POST',headers={{name='X-Token',value='test-secret'}},body=string.char(0,255)..'hi'})
            assert(response.status==302)
            assert(response.body==string.char(0,255)..'ok')
            local values={}
            for _,header in ipairs(response.headers)do if header.name=='x-test' then values[#values+1]=header.value end end
            assert(#values==2 and values[1]=='a' and values[2]=='b')
            margatroid.task:return_result('http passed')
        end
    "#).unwrap();
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(ClosurePlugin)
        .add_closure_system(RuntimePlugin::UPDATE)
        .add_plugin(ToolPlugin::new().with_runner_path(env!("CARGO_BIN_EXE_tool_runner")));
    let owner = app.world_mut().spawn();
    app.world_mut().insert_component(owner, ToolTasks::new());
    let (sender, mut reply) = oneshot::channel();
    app.world().emit_event(ToolExecuteRequest::Path {
        initiator: owner,
        path: root.path().join("tool.toml"),
        arguments: json!({"url":url}),
        reply: Arc::new(Mutex::new(Some(sender))),
    });
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        app.tick();
        if let Ok(result) = reply.try_recv() {
            let result = result.unwrap();
            assert!(!result.failed, "{}", result.content);
            assert_eq!(result.content, "http passed");
            break;
        }
        assert!(Instant::now() < deadline, "worker HTTP result timeout");
        std::thread::sleep(Duration::from_millis(2));
    }
    while !app
        .world()
        .get_component::<ToolTasks>(owner)
        .unwrap()
        .list()
        .is_empty()
    {
        app.tick();
        assert!(Instant::now() < deadline, "worker failed to drain");
        std::thread::sleep(Duration::from_millis(2));
    }
    let (header, body) = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(header.starts_with("POST /request HTTP/1.1"));
    assert!(header.to_ascii_lowercase().contains("x-token: test-secret"));
    assert_eq!(body, b"\x00\xffhi");
    server.join().unwrap();
}
