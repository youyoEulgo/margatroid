# server_plugin

## 介绍

`server_plugin` 将 Axum HTTP 与 WebSocket 服务接入 mecs。它负责连接、路由、流式通道、
背压和生命周期；业务协议仍由其他 Plugin 的 System 处理。

Axum 持有协议状态、连接和网络通道，ECS 持有同步状态并做业务编排。完整消息交给同步 System；
需要等待的数据留在异步线程累积或转发。流分片不逐条进入事件队列，避免网络速率改变 ECS 帧语义。

这里采用同步处理、异步累积：事件传递连接和流的状态，通道传递持续到达的数据。发送器可以
复制给多个持有者，接收器只能转交；流 ID 确定支流，连接 ID 确定客户端。Axum 管理网络，
ECS 编排业务。

`WebSocketMessageSender` 是不经过事件队列的直接发送终端。调用方先从 `WebSocketConnections`
解析并固定一组 `WebSocketSender`，再与已经构造好的 `WebSocketMessage` 组成发送终端。同步 System
使用 `try_send`，异步流式任务使用可等待的 `send`；该类型不解析业务 target，也不序列化业务协议。

## 使用说明

```rust
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use axum::{routing::get, Router};
use core_plugin::App;
use server_plugin::{AppServerExt, ServerPlugin};

let mut app = App::new();
app.add_plugin(RuntimePlugin::default())
    .add_plugin(AsyncRuntimePlugin)
    .add_plugin(ServerPlugin::default())
    .add_http_routes(Router::new().route("/health", get(|| async { "ok" })));
```

默认监听 `127.0.0.1:3939`。原生 Axum Router 适合健康检查、中间件、文件上传等
不需要 ECS 委托的路由。

## HTTP事件委托

```rust
use app_runtime_plugin::RuntimePlugin;
use axum::body::Bytes;
use axum::http::{Method, Response};
use core_plugin::World;
use server_plugin::HttpRequestReceived;

app.add_http_event_route(Method::POST, "/prompt")
    .add_system(RuntimePlugin::UPDATE, |world: &mut World| {
        for request in world.event_reader::<HttpRequestReceived>() {
            request
                .respond(Response::new(Bytes::from_static(b"accepted")))
                .unwrap();
        }
    });
```

普通响应调用 `respond` 一次完成。流式响应使用同一个可克隆会话：

```text
request.start_stream(head)
→ response_session.send_chunk(...).await
→ response_session.send_chunk(...).await
→ response_session.finish()
```

会话可以在 System、事件和异步任务之间反复转交，适合 LLM 流式响应与 tool call 循环。

### 使用异步闭包发送HTTP流

同步 System 只负责读取请求事件、开始响应并发出异步请求。等待上游数据和发送分片都在
AsyncRuntime 中执行：

```rust
use anyhow::Error;
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::WorldAsyncExt;
use axum::body::Bytes;
use axum::http::Method;
use closure_plugin::{AppClosureExt, ClosurePlugin};
use core_plugin::World;
use server_plugin::{AppServerExt, HttpRequestReceived, HttpResponseHead};

app.add_plugin(ClosurePlugin)
    .add_closure_system(RuntimePlugin::PRE_UPDATE)
    .add_http_event_route(Method::POST, "/stream")
    .add_system(RuntimePlugin::UPDATE, start_http_streams);

fn start_http_streams(world: &mut World) {
    let responses = world
        .event_reader::<HttpRequestReceived>()
        .into_iter()
        .map(|request| {
            // 这里只建立响应通道，不执行等待操作。
            request.start_stream(HttpResponseHead::default()).unwrap();
            request.response_session()
        })
        .collect::<Vec<_>>();

    for response in responses {
        world.send_async_closure(
            RuntimePlugin::PRE_UPDATE,
            move || async move {
                for chunk in ["hello ", "world"] {
                    response.send_chunk(Bytes::from(chunk)).await?;
                }
                response.finish()?;
                Ok::<(), Error>(())
            },
        );
    }
}
```

这里显式挂载的 `ClosureSystem` 负责在 `PRE_UPDATE` 取得同步包装闭包；包装闭包随后把真正的
异步任务交给专用线程，并在完成后产生 `Result<(), Error>` 响应事件。若一次 LLM 调用结束后需要同步判断 tool call，
可以让异步任务返回累积后的响应类型，再由另一个同步 System 读取对应的 `Result` 事件。

## WebSocket

```rust
app.add_websocket_event_route("/ws");
```

不含 `mecs_stream` 信封的文本或二进制消息产生 `WebSocketMessageReceived` 事件。
事件只携带连接 ID 与消息；System 从 `WebSocketConnections` Resource 取得可克隆的
`WebSocketSender`，同步 System 可以 `try_send`，异步任务可以 `send(...).await`。

默认 JSON 支流信封：

```json
{
  "mecs_stream": {
    "id": "prompt-1",
    "phase": "start"
  },
  "payload": {}
}
```

`phase` 支持 `start`、`chunk`、`end` 和 `abort`。`start` 只产生一次
`WebSocketStreamOpened` 事件；后续分片进入有界支流通道，不会逐条堆入 ECS 事件队列。
System 通过事件句柄取出唯一的 `WebSocketStreamReceiver`，再把它移动进异步任务累积。

### Axum主动分流与异步累积

每个客户端连接都有独立的 `WebSocketConnectionId`。当某个客户端发送一个新的 `start`
信封时，Axum 根据 `(connection_id, stream_id)` 创建有界支流通道，把唯一读取器包装进
`WebSocketStreamOpened` 事件。之后同一支流的 `chunk`、`end` 和 `abort` 由 Axum 直接
写入该通道，不再为每个分片发送 ECS 事件。

同步 System 取得读取器后，可以通过异步事件持续累积或逐条处理：

```rust
use anyhow::Error;
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::WorldAsyncExt;
use closure_plugin::{AppClosureExt, ClosurePlugin};
use core_plugin::World;
use server_plugin::{
    WebSocketConnectionId, WebSocketMessage, WebSocketStreamId, WebSocketStreamOpened,
};

struct ReceivedStream {
    connection_id: WebSocketConnectionId,
    stream_id: WebSocketStreamId,
    messages: Vec<WebSocketMessage>,
}

app.add_plugin(ClosurePlugin)
    .add_closure_system(RuntimePlugin::PRE_UPDATE)
    .add_system(RuntimePlugin::UPDATE, accumulate_websocket_streams);

fn accumulate_websocket_streams(world: &mut World) {
    let streams = world
        .event_reader::<WebSocketStreamOpened>()
        .into_iter()
        .filter_map(|event| {
            Some((
                event.connection_id,
                event.stream_id.clone(),
                event.receiver.take()?,
            ))
        })
        .collect::<Vec<_>>();

    for (connection_id, stream_id, mut receiver) in streams {
        world.send_async_closure(
            RuntimePlugin::PRE_UPDATE,
            move || async move {
                let mut messages = Vec::new();
                while let Some(message) = receiver.recv().await {
                    // 也可以在这里逐条解析、落盘或转发，不必等到结束再处理。
                    messages.push(message?);
                }
                Ok::<_, Error>(ReceivedStream {
                    connection_id,
                    stream_id,
                    messages,
                })
            },
        );
    }
}
```

异步任务完成后产生 `Result<ReceivedStream, Error>` 事件，后续同步 System 可以继续处理
完整结果。收到 `abort` 或连接提前断开时，`receiver.recv()` 返回相应错误。

### 取得并命名客户端发送器

ServerPlugin 支持任意数量的并发 WebSocket 客户端，并自动维护
`WebSocketConnections` Resource。连接建立后，Axum 先把发送器以空名称写入注册表，再
发送只携带 `connection_id` 的 `WebSocketConnected` 通知事件；连接断开时先自动删除
注册表条目，再发送 `WebSocketDisconnected` 通知事件。用户不需要编写 System 保存或
清理发送器。

新连接默认是不具名发送器。用户可以在连接通知到达时按 ID 设置全局唯一名称：

```rust
use core_plugin::World;
use server_plugin::{WebSocketConnected, WebSocketConnections};

fn name_new_connections(world: &mut World) {
    let connection_ids = world
        .event_reader::<WebSocketConnected>()
        .into_iter()
        .map(|event| event.connection_id)
        .collect::<Vec<_>>();
    let connections = world.get_resource::<WebSocketConnections>().unwrap();

    for connection_id in connection_ids {
        // 实际业务可以把这里替换为鉴权得到的用户、Agent或Workspace名称。
        let name = format!("connection-{}", connection_id.get());
        match connections.set_name(connection_id, name) {
            Ok(()) => {}
            Err(error) => tracing::warn!(%error, "WebSocket连接命名失败"),
        }
    }
}
```

名称为空表示不具名；对已具名连接调用 `set_name(connection_id, "")` 会清除名称。
命名时会在注册表写锁内查重。可以使用 `get(connection_id)` 按 ID 查找、使用
`get_by_name(name)` 按唯一名称查找，或者使用 `unnamed()` 取得全部不具名发送器。
连接可能在通知被处理前已经断开，此时查找返回 `None`，直接忽略即可。

单次定向发送可以留在同步 System 中：

```rust
use axum::extract::ws::Message;
use core_plugin::World;
use server_plugin::{
    WebSocketConnectionId, WebSocketConnections, WebSocketSendError,
};

fn send_once(
    world: &World,
    connection_id: WebSocketConnectionId,
) -> Result<(), WebSocketSendError> {
    let sender = world
        .get_resource::<WebSocketConnections>()
        .and_then(|connections| connections.get(connection_id))
        .ok_or(WebSocketSendError::ConnectionClosed)?;

    sender.try_send(Message::Text("done".into()))
}
```

持续等待数据并向指定客户端流式发送时，克隆发送器并交给异步事件。WebSocket 的每条
消息本身就是一个完整分片；`start/chunk/end` 是业务选择的出站信封，ServerPlugin 不会
替业务生成：

```rust
use anyhow::Error;
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::WorldAsyncExt;
use axum::extract::ws::Message;
use core_plugin::World;
use server_plugin::{WebSocketConnectionId, WebSocketConnections};

fn stream_to_client(world: &World, connection_id: WebSocketConnectionId) {
    let Some(sender) = world
        .get_resource::<WebSocketConnections>()
        .and_then(|connections| connections.get(connection_id))
    else {
        return;
    };

    world.send_async_closure(
        RuntimePlugin::PRE_UPDATE,
        move || async move {
            for message in [
                r#"{"mecs_stream":{"id":"reply-1","phase":"start"}}"#,
                r#"{"mecs_stream":{"id":"reply-1","phase":"chunk"},"payload":"hello"}"#,
                r#"{"mecs_stream":{"id":"reply-1","phase":"end"}}"#,
            ] {
                sender.send(Message::Text(message.into())).await?;
            }
            Ok::<(), Error>(())
        },
    );
}
```

多客户端之间的连接通道互相隔离；同一客户端的多个发送器克隆汇入该连接自己的有界
写入通道。简单身份可以直接使用连接名称；需要一个用户对应多个连接等复杂关系时，业务
Plugin 仍可另外维护自己的索引。

二进制流或其他信封格式可以通过 `add_websocket_event_route_with` 注册自定义
`WebSocketMessageClassifier`。

完整伪代码和边界说明见 [DESIGN.md](DESIGN.md)。
