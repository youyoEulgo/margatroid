# axum_plugin

`axum_plugin` 为 Margatroid 应用装配原生 Axum HTTP 服务。接入方提交已绑定 State 的 `axum::Router`，插件合并路由、启动监听，并提供覆盖 HTTP 请求与升级连接的服务生命周期句柄。

## 装配一个 HTTP 入口

```rust
use app_runtime_plugin::{AppRunExt, RuntimePlugin};
use async_runtime_plugin::AsyncRuntimePlugin;
use axum::{routing::get, Router};
use axum_plugin::{AppAxumExt, AxumPlugin};
use closure_plugin::{AppClosureExt, ClosurePlugin};
use core_plugin::App;

fn main() {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(ClosurePlugin)
        .add_closure_system(RuntimePlugin::UPDATE)
        .add_plugin(AxumPlugin::bind("127.0.0.1:3000".parse().unwrap()))
        .add_axum_routes(Router::new().route("/health", get(|| async { "ok" })));
    app.run();
}
```

在首次 tick 前完成路由装配。STARTUP 取走合并后的 Router，并向 UPDATE 闭包执行器提交异步服务任务。监听成功后，`AxumHandle::local_address()` 提供实际监听地址；绑定端口为 `0` 时，可由此取得系统分配的端口。

## 服务对象

- `AxumPlugin`：通过 `bind(SocketAddr)` 选择监听地址。
- `AppAxumExt`：通过 `add_axum_routes(Router)` 合并原生路由。
- `AxumHandle`：查询地址、请求停止、等待完成并读取最终结果。
- `AxumLifetime`：向 handler 提供共享停止信号与工作登记入口。
- `AxumWorkGuard`：通过 RAII 将升级连接及其工作纳入服务完成条件。
- `AxumError` / `AxumErrorKind`：表达安装、路由冻结、绑定和运行错误。

handler 沿用 Axum 的响应、State、Extension 与提取器接口。服务通过 `ConnectInfo<SocketAddr>` 提供真实对端地址，并将 `AxumLifetime` 注入 Extension。

## 关闭 HTTP 与升级连接

通过 World 取得并克隆 `AxumHandle`，调用 `request_shutdown()` 广播停止信号。普通 HTTP 请求完成后，服务继续等待已登记的工作；`wait_stopped().await` 返回最终 `Result<(), AxumError>`，同步调用方可查询 `is_stopped()` 与 `result()`。

WebSocket handler 在 `on_upgrade` 前调用 `lifetime.track()`，将 guard 移入升级回调，并持有到 socket 读写与子任务实际结束。工作可通过 `lifetime.cancelled().await` 接收停止信号。HTTP graceful shutdown 和全部 guard 释放共同构成服务完成条件。

App 释放时请求服务停止。应用级关闭流程保持 World 推进，等待领域任务与服务实际收口后释放资源。完整的请求、升级连接及停止示例见[生命周期测试](tests/lifecycle.rs)。

## 继续阅读

- [公开接口与句柄](src/lib.rs)、[启动与结果发布](src/system.rs)、[监听与关闭流程](src/handler.rs)。
- [生命周期测试](tests/lifecycle.rs)：HTTP 响应、升级工作登记、绑定失败和路由冻结。
- [设计与完整接口](DESIGN.md)。
