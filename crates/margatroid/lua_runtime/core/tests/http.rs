use lua_runtime_core::host::{
    HostDefinition, HostKind, Margatroid,
    http::{HttpContext, HttpErrorKind, HttpHeader, HttpRequestOptions, install_http},
};
use mlua::{Lua, Value};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::mpsc,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;
fn options(url: String) -> HttpRequestOptions {
    HttpRequestOptions {
        url,
        method: "GET".into(),
        headers: vec![],
        body: None,
        timeout_ms: 1000,
        max_response_bytes: 8 * 1024 * 1024,
    }
}
fn server(response: Vec<u8>) -> (String, mpsc::Receiver<Vec<u8>>, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    let join = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut request = Vec::new();
        let mut byte = [0];
        while !request.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
        }
        let text = String::from_utf8_lossy(&request);
        let length = text
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .and_then(|v| v.trim().parse::<usize>().ok())
            })
            .unwrap_or(0);
        let mut body = vec![0; length];
        stream.read_exact(&mut body).unwrap();
        request.extend(body);
        let _ = tx.send(request);
        let _ = stream.write_all(&response);
    });
    (url, rx, join)
}
fn response(status: &str, headers: &str, body: &[u8]) -> Vec<u8> {
    let mut result = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n",
        body.len()
    )
    .into_bytes();
    result.extend(body);
    result
}
#[tokio::test]
async fn http_preserves_binary_body_headers_and_error_status() {
    let (url, rx, join) = server(response(
        "422 Unprocessable Entity",
        "X-Repeat: first\r\nX-Repeat: second\r\n",
        b"\0\xffbinary",
    ));
    let context = HttpContext::new(CancellationToken::new(), None).unwrap();
    let mut request = options(url);
    request.method = "POST".into();
    request.body = Some(vec![0, 255, 3]);
    request.headers = vec![HttpHeader {
        name: "Authorization".into(),
        value: b"Bearer secret".to_vec(),
    }];
    let result = context.request(request).await.unwrap();
    assert_eq!(result.status, 422);
    assert_eq!(result.body, b"\0\xffbinary");
    assert_eq!(
        result
            .headers
            .iter()
            .filter(|h| h.name == "x-repeat")
            .map(|h| h.value.clone())
            .collect::<Vec<_>>(),
        vec![b"first".to_vec(), b"second".to_vec()]
    );
    let sent = rx.recv().unwrap();
    assert!(sent.ends_with(&[0, 255, 3]));
    assert!(String::from_utf8_lossy(&sent).contains("accept-encoding: identity"));
    join.join().unwrap();
    context.close_and_wait().await;
}
#[tokio::test]
async fn lua_byte_strings_and_readonly_phase_gate() {
    let (url, rx, join) = server(response("200 OK", "", &[255, 0, 1]));
    let lua = Lua::new();
    let context = HttpContext::new(CancellationToken::new(), None).unwrap();
    let host = Margatroid::install(
        &lua,
        HostDefinition {
            kind: HostKind::Tool,
            context: serde_json::json!({}),
            arguments: serde_json::json!({}),
        },
    )
    .unwrap();
    assert!(
        lua.load("margatroid.http:request({url='http://localhost'})")
            .exec_async()
            .await
            .is_err()
    );
    assert!(lua.load("margatroid.http={}").exec().is_err());
    let callbacks = lua.create_table().unwrap();
    callbacks
        .set("http_request", install_http(&lua, context.clone()).unwrap())
        .unwrap();
    Margatroid::enable(&host, callbacks).unwrap();
    lua.globals().set("url", url).unwrap();
    lua.load("local r=margatroid.http:request({url=url,method='POST',body=string.char(0,255)});assert(r.status==200);assert(r.body==string.char(255,0,1));assert(type(r.headers)=='table')").exec_async().await.unwrap();
    assert!(rx.recv().unwrap().ends_with(&[0, 255]));
    join.join().unwrap();
    context.close_and_wait().await;
}
#[tokio::test]
async fn invalid_options_are_rejected_before_network_without_secrets() {
    let lua = Lua::new();
    let context = HttpContext::new(CancellationToken::new(), None).unwrap();
    lua.globals()
        .set("request", install_http(&lua, context.clone()).unwrap())
        .unwrap();
    for expression in [
        "{url='ftp://host'}",
        "{url='http://@localhost'}",
        "{url='https://secret:password@host'}",
        "{url='https://host/#secret'}",
        "{url='http://localhost',method='get'}",
        "{url='http://localhost',body=''}",
        "{url='http://localhost',redirect=true}",
        "{url='http://localhost',headers={[2]={name='X',value='x'}}}",
        "{url='http://localhost',headers={bad=true}}",
        "{url='http://localhost',headers={{name='X',value=12}}}",
        "{url='http://localhost',headers={{name='Host',value='bad'}}}",
        "{url='http://localhost',headers={{name='X',value='a\\r\\nb'}}}",
        "{url='http://localhost',timeout_ms=120001}",
        "{url='http://localhost',max_response_bytes=0}",
        "{url='http://localhost',max_response_bytes=16777217}",
    ] {
        let error = lua
            .load(format!("return request({expression})"))
            .eval_async::<Value>()
            .await
            .unwrap_err()
            .to_string();
        assert!(!error.contains("password"));
        assert!(
            error.contains("InvalidRequest") || error.contains("HeadersTooLarge"),
            "{expression}: {error}"
        );
    }
    context.close_and_wait().await;
}
#[tokio::test]
async fn redirects_and_compressed_entities_are_not_followed_or_decoded() {
    let raw = [31, 139, 0, 255];
    let (url, _, join) = server(response(
        "302 Found",
        "Location: http://127.0.0.1:1/secret\r\nContent-Encoding: gzip\r\n",
        &raw,
    ));
    let context = HttpContext::new(CancellationToken::new(), None).unwrap();
    let result = context.request(options(url)).await.unwrap();
    assert_eq!(result.status, 302);
    assert_eq!(result.body, raw);
    join.join().unwrap();
}
#[tokio::test]
async fn response_limits_apply_to_declared_and_chunked_lengths() {
    for response in [response("200 OK","",b"too long"),b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n8\r\ntoo long\r\n0\r\n\r\n".to_vec()] {
  let(url,_,join)=server(response);let context=HttpContext::new(CancellationToken::new(),None).unwrap();let mut request=options(url);request.max_response_bytes=3;assert_eq!(context.request(request).await.unwrap_err().kind,HttpErrorKind::ResponseTooLarge);join.join().unwrap();
 }
}
#[tokio::test]
async fn timeout_deadline_cancel_and_close_release_requests() {
    let context = HttpContext::new(
        CancellationToken::new(),
        Some(Instant::now() - Duration::from_secs(1)),
    )
    .unwrap();
    assert_eq!(
        context
            .request(options("http://127.0.0.1:1".into()))
            .await
            .unwrap_err()
            .kind,
        HttpErrorKind::Timeout
    );
    let cancel = CancellationToken::new();
    let context = HttpContext::new(cancel.clone(), None).unwrap();
    cancel.cancel();
    assert_eq!(
        context
            .request(options("http://127.0.0.1:1".into()))
            .await
            .unwrap_err()
            .kind,
        HttpErrorKind::Cancelled
    );
    context.close_and_wait().await;
    assert_eq!(
        context
            .request(options("http://127.0.0.1:1".into()))
            .await
            .unwrap_err()
            .kind,
        HttpErrorKind::Closed
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let context = HttpContext::new(CancellationToken::new(), None).unwrap();
    let mut request = options(url);
    request.timeout_ms = 20;
    let outcome = context.request(request).await;
    assert_eq!(outcome.unwrap_err().kind, HttpErrorKind::Timeout);
    context.close_and_wait().await;
}
#[tokio::test]
async fn concurrency_bound_and_dropped_futures_release_admission() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let context = HttpContext::new(CancellationToken::new(), None).unwrap();
    let mut active = Vec::new();
    for _ in 0..8 {
        let future = Box::pin(context.request(options(url.clone())));
        active.push(future);
    }
    for future in &mut active {
        assert!(futures_util::poll!(future.as_mut()).is_pending());
    }
    assert_eq!(
        context
            .request(options(url.clone()))
            .await
            .unwrap_err()
            .kind,
        HttpErrorKind::Busy
    );
    active.pop();
    let mut admitted = Box::pin(context.request(options(url)));
    assert!(futures_util::poll!(admitted.as_mut()).is_pending());
    drop(active);
    drop(admitted);
    tokio::time::timeout(Duration::from_secs(1), context.close_and_wait())
        .await
        .unwrap();
}

#[tokio::test]
async fn head_and_header_and_request_size_limits() {
    let (url, _, join) = server(
        b"HTTP/1.1 200 OK\r\nContent-Length: 99999999\r\nConnection: close\r\n\r\n".to_vec(),
    );
    let context = HttpContext::new(CancellationToken::new(), None).unwrap();
    let mut request = options(url);
    request.method = "HEAD".into();
    assert!(context.request(request).await.unwrap().body.is_empty());
    join.join().unwrap();
    let mut request = options("http://127.0.0.1:1".into());
    request.method = "POST".into();
    request.body = Some(vec![0; 8 * 1024 * 1024 + 1]);
    assert_eq!(
        context.request(request).await.unwrap_err().kind,
        HttpErrorKind::InvalidRequest
    );
    let mut request = options("http://127.0.0.1:1".into());
    request.headers = vec![
        HttpHeader {
            name: "x-many".into(),
            value: b"v".to_vec()
        };
        129
    ];
    assert_eq!(
        context.request(request).await.unwrap_err().kind,
        HttpErrorKind::HeadersTooLarge
    );
    let mut request = options("http://127.0.0.1:1".into());
    request.headers = vec![HttpHeader {
        name: "x-long".into(),
        value: vec![b'a'; 65536],
    }];
    assert_eq!(
        context.request(request).await.unwrap_err().kind,
        HttpErrorKind::HeadersTooLarge
    );
    let headers = "X-Test: ok\r\n".repeat(129);
    let (url, _, join) = server(response("200 OK", &headers, b""));
    // Hyper may reject the oversized header section before our response check.
    assert!(matches!(
        context.request(options(url)).await.unwrap_err().kind,
        HttpErrorKind::HeadersTooLarge | HttpErrorKind::Transport
    ));
    join.join().unwrap();
}

#[tokio::test]
async fn close_cancels_actual_waiting_request_and_never_parent_execution() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let parent = CancellationToken::new();
    let context = HttpContext::new(parent.clone(), None).unwrap();
    let request = context.request(options(url));
    let close = async {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut byte = [0];
        stream.read_exact(&mut byte).await.unwrap();
        context.close_and_wait().await;
        assert!(!parent.is_cancelled());
        let _ = stream.shutdown().await;
    };
    let (result, ()) = tokio::join!(request, close);
    assert_eq!(result.unwrap_err().kind, HttpErrorKind::Cancelled);
}
