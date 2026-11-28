# client_plugin

`client_plugin` 在 mecs 中安装客户端登记与断连系统。
它使用 client_core 的共享 Clients 槽位，并为成功登记的连接建立 Client 与 Address 组件。

## 核心入口

- `ClientPlugin::new()` 使用 RuntimePlugin 的 UPDATE 调度。
- `with_schedule` 选择应用已建立的调度。
- `RegisterClient` 携带请求标识、连接标识、客户端类型、可选名称和 oneshot 回复。
- `ClientDisconnected` 驱动对应 Client 实体的清理。

## 安装并登记一个连接

下面延续网络入口已接受连接的流程。示例可放入依赖 `client_plugin`、
`core_plugin`、`app_runtime_plugin` 和 Tokio 的包中。

```rust
use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use client_plugin::{ClientPlugin, RegisterClient, WorldClientExt};
use core_plugin::App;
use std::sync::{Arc, Mutex};

fn main() {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(ClientPlugin::new());
    let clients = app.world().clients();
    let (connection, _receiver, _cancel) = clients.allocate().unwrap();
    let (reply, mut result) = tokio::sync::oneshot::channel();
    app.world().send_event(RegisterClient {
        id: "register-1".into(),
        connection_id: connection,
        client_type: "webui".into(),
        name: Some("console".into()),
        reply: Arc::new(Mutex::new(Some(reply))),
    });
    app.tick();
    app.tick();
    let registered = result.try_recv().unwrap().unwrap();
    assert!(app.world().client_by_connection(connection).is_some());
    assert!(app.world().is_alive(registered.client));
}
```

ClientPlugin 使用 World 已有的 Clients resource；应用也可在安装前放入共享实例。
名称省略时使用连接数字标识，名称按整个注册表检查唯一性。
成功身份地址形如 `/clients/webui/console/1`，实际末段取连接标识。
重复登记、重复名称、地址冲突或已关闭连接通过 ClientError 返回。

连接退出时，传输层移除共享槽位并发送 ClientDisconnected。
登记与断连竞争、失败回滚和默认名称示例见 [注册测试](tests/registration.rs)。

## 查阅与验证

- [插件入口](src/lib.rs)：共享 resource 与系统安装。
- [处理函数](src/handler.rs)：身份校验、登记提交和断连清理。
- [系统](src/system.rs)：事件消费。
- [core 使用说明](../core/README.md) 与 [领域设计](../DESIGN.md)。

运行 `cargo test -p client_plugin` 验证登记与寻址。
