# AsyncRuntimePlugin

`AsyncRuntimePlugin` 为 mecs 提供专用异步线程，不把 Tokio 引入 `core_plugin`。

## 快速开始

```rust
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::{
    AppAsyncExt, AsyncRuntimePlugin, AsyncTaskError, WorldAsyncExt,
};
use core_plugin::{App, Plugin, World};

#[derive(Debug)]
enum LoadError {
    Io(std::io::Error),
    AsyncTask(AsyncTaskError),
}

impl From<AsyncTaskError> for LoadError {
    fn from(error: AsyncTaskError) -> Self {
        Self::AsyncTask(error)
    }
}

struct LoadPlugin;

impl Plugin for LoadPlugin {
    fn build(self, app: &mut App) {
        app.add_async_system::<String, LoadError>(RuntimePlugin::UPDATE)
            .add_system(RuntimePlugin::UPDATE, handle_load_results);
    }
}

async fn load() -> Result<String, LoadError> {
    Ok("done".to_string())
}

fn handle_load_results(world: &mut World) {
    for _result in world.event_reader::<Result<String, LoadError>>() {
        // 处理异步结果
    }
}

let mut app = App::new();
app.add_plugin(RuntimePlugin::default())
    .add_plugin(AsyncRuntimePlugin)
    .add_plugin(LoadPlugin);

app.world().send_async_event(
    load,
    false,
);
```

### 传递参数

`send_async_event` 不会自动填充业务参数。有业务参数的异步函数通过闭包捕获参数：

```rust
async fn load_agent(name: String) -> Result<String, LoadError> {
    Ok(format!("loaded {name}"))
}

let name = "reviewer".to_string();
app.world()
    .send_async_event(move || load_agent(name), false);
```

闭包和它捕获的值都必须满足 `Send + 'static`，异步任务不能借用 `World`。

### 在任务中发送事件

异步函数声明 `AsyncContext` 参数后，AsyncRuntime 会自动传入上下文，不需要调用方手动
创建或捕获事件发送器：

```rust
use async_runtime_plugin::AsyncContext;
use core_plugin::Event;

struct LoadProgress(u8);
impl Event for LoadProgress {}

async fn load_with_progress(context: AsyncContext) -> Result<String, LoadError> {
    context.send_event(LoadProgress(25));
    context.send_event(LoadProgress(75));
    Ok("done".to_string())
}

app.register_event::<LoadProgress>();
app.world()
    .send_async_event(load_with_progress, false);
```

`AsyncContext::send_event` 每次都会写入事件队列并唤醒 Runtime。普通异步请求的中间事件
可以在任务完成前处理；阻塞异步请求需要等任务完成并开阀后处理。

### 使用 anyhow

不需要业务错误枚举时，可以直接将 `anyhow::Error` 注册为错误类型：

```rust
use anyhow::{Context, Result};

async fn load_config() -> Result<String> {
    tokio::fs::read_to_string("margatroid-workspace.yaml")
        .await
        .context("加载 Workspace 配置失败")
}

app.add_async_system::<String, anyhow::Error>(RuntimePlugin::UPDATE);
app.world().send_async_event(load_config, false);
```

业务错误和异步任务 panic、取消产生的 `AsyncTaskError` 都会作为
`Result<String, anyhow::Error>` 响应事件返回。

## 公开 API

- `AsyncRuntimePlugin`：启动并管理专用异步线程。
- `AsyncContext`：由 AsyncRuntime 自动注入，允许异步任务发送一个或多个普通事件。
- `AsyncTask`：同时适配无上下文函数和接收 `AsyncContext` 的函数。
- `AsyncRequest<T, E>`：封装一次只能执行一次的异步闭包。
- `AsyncRequest::new`：普通请求，完成后唤醒 Runtime。
- `AsyncRequest::blocking`：阻止下一帧开始，完成后开阀。
- `AppAsyncExt::add_async_system`：注册请求与响应事件，并将异步分发 System 挂到开发者指定的 Schedule。
- `WorldAsyncExt::send_async_event`：从异步闭包构造并发送 `AsyncRequest`，唤醒 Runtime，
  布尔参数决定是否阻塞下一帧。
- `AsyncTaskError`：将任务 panic 或取消转换进开发者选择的错误类型 `E`。
- `AsyncRuntimeError`：报告异步基础设施的配置与运行错误。

每个请求独立 `tokio::spawn`。异步任务不借用 `World`；完成结果通过
`Result<T, E>` 事件返回主线程。请求 ID、Agent ID 和响应路由由开发者定义。

`add_async_system::<T, E>(schedule)` 挂载的是读取 `AsyncRequest<T, E>` 的通用异步分发
System，不是最终的业务响应 System。业务 Plugin 仍需自行添加读取
`Result<T, E>` 的 System，并决定它所属的 Schedule。
