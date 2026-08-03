# AsyncRuntimePlugin

## 介绍

`AsyncRuntimePlugin` 为 mecs 提供专用异步线程，不把 Tokio 引入 `core_plugin`。同步 System
始终在主线程运行；异步线程只执行不能在一帧内完成的任务，结果以 `Result<T, E>` 事件返回。

异步有两种入口：

- 事件模式：事件传数据，预先挂载的异步 System 持有固定处理闭包。
- 闭包模式：请求传一次性异步闭包，通过 `ClosurePlugin` 的同步闭包 System 提交任务。

两种模式都会先创建 pending 响应。任务完成后 pending 升变为普通事件；panic 或取消会转换为
`AsyncTaskError`，再进入开发者选择的错误类型 `E`。

## 事件模式

适合一种请求需要长期复用同一种处理逻辑的场景：

```rust
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::{AppAsyncExt, AsyncRuntimePlugin, AsyncTaskError, WorldAsyncExt};
use core_plugin::{App, Event, Plugin};

struct LoadAgent {
    name: String,
}
impl Event for LoadAgent {}

#[derive(Debug)]
enum LoadError {
    NotFound,
    AsyncTask(AsyncTaskError),
}

impl From<AsyncTaskError> for LoadError {
    fn from(error: AsyncTaskError) -> Self {
        Self::AsyncTask(error)
    }
}

async fn load_agent(request: LoadAgent) -> Result<String, LoadError> {
    Ok(format!("loaded {}", request.name))
}

let mut app = App::new();
app.add_plugin(RuntimePlugin::default())
    .add_plugin(AsyncRuntimePlugin)
    .add_async_system(RuntimePlugin::UPDATE, load_agent);

app.world().send_async_event(LoadAgent {
    name: "reviewer".into(),
});
```

`send_async_event` 只发送数据。`add_async_system` 决定事件类型、固定异步处理闭包和处理阶段。
同一请求事件类型最多绑定一个异步 System。

需要阻止下一帧开始时使用 `send_await_event(request)`。请求被异步 System 真正取得后才关阀，
当前帧仍会完成；任务结束后开阀。

## 闭包模式

一次性文件操作、网络请求或临时组合任务可以直接发送闭包。闭包模式依赖 `ClosurePlugin`，并且
开发者必须显式选择允许执行闭包的 Schedule：

```rust
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::{AsyncRuntimePlugin, WorldAsyncExt};
use closure_plugin::{AppClosureExt, ClosurePlugin};
use core_plugin::App;

let mut app = App::new();
app.add_plugin(RuntimePlugin::default())
    .add_plugin(ClosurePlugin)
    .add_closure_system(RuntimePlugin::PRE_UPDATE)
    .add_plugin(AsyncRuntimePlugin);

let path = "margatroid-workspace.yaml".to_string();
app.world().send_async_closure(
    RuntimePlugin::PRE_UPDATE,
    move || async move {
        tokio::fs::read_to_string(path)
            .await
            .map_err(anyhow::Error::from)
    },
);
```

`send_async_closure` 不阻塞下一帧；`send_await_closure` 在闭包被 `ClosureSystem` 取得并开始提交
异步任务时关阀。异步插件不会另外挂载 Dispatcher，也不会复制 ClosurePlugin 的路由逻辑。

## 异步上下文

事件 handler 和一次性闭包都可以声明 `AsyncContext` 参数，由插件自动注入：

```rust
use async_runtime_plugin::AsyncContext;
use core_plugin::Event;

struct LoadProgress(u8);
impl Event for LoadProgress {}

struct LoadWithProgress {
    name: String,
}
impl Event for LoadWithProgress {}

async fn load_with_progress(
    request: LoadWithProgress,
    context: AsyncContext,
) -> Result<String, LoadError> {
    context.send_event(LoadProgress(25));
    context.send_event(LoadProgress(75));
    Ok(request.name)
}

app.add_async_system(RuntimePlugin::UPDATE, load_with_progress);
```

`AsyncContext` 不持有 `World`，只提供跨线程安全的能力。它发送的每个事件都会经过 Runtime
事件发送器并唤醒 Runtime。

## 使用 anyhow

不需要自定义业务错误枚举时，可以直接使用 `anyhow::Error`。它可以接收业务错误以及框架产生的
`AsyncTaskError`：

```rust
use anyhow::{Context, Result};

async fn load_config(_request: LoadAgent) -> Result<String> {
    tokio::fs::read_to_string("margatroid-workspace.yaml")
        .await
        .context("加载Workspace配置失败")
}

app.add_async_system(RuntimePlugin::UPDATE, load_config);
```

## 长期服务

`spawn_async_service` 直接提交长期 Future，供 Server 等基础设施 Plugin 管理服务生命周期。
它不创建请求事件、pending 响应或完成事件，也不操作 Runtime 阀。

## 公开 API

- `AsyncRuntimePlugin`：启动并管理专用异步线程。
- `AppAsyncExt::add_async_system`：绑定请求事件类型、固定异步 handler 与 Schedule。
- `WorldAsyncExt::send_async_event`、`send_await_event`：发送事件模式请求。
- `WorldAsyncExt::send_async_closure`、`send_await_closure`：发送闭包模式请求。
- `WorldAsyncExt::spawn_async_service`：提交由所属 Plugin 自行管理的长期 Future。
- `AsyncEventHandler`：适配有或没有 `AsyncContext` 的固定 handler。
- `AsyncTask`：适配有或没有 `AsyncContext` 的一次性异步闭包。
- `AsyncContext`：在异步任务中发送普通事件。
- `AsyncTaskError`：描述任务 panic 或取消。

请求 ID、Agent ID 和同类型并发响应路由属于业务数据，由开发者自行携带。
