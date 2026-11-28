# async_runtime_plugin

`async_runtime_plugin` 在专用线程上运行 Tokio 任务，并将 `Result<T, E>` 交回 ECS 事件流。同步 System 负责访问 World，异步任务持有请求数据，适合文件、网络与后台计算的接入。

## 注册请求、提交任务、接收结果

```rust
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use app_runtime_plugin::{AppRunExt, RuntimePlugin};
use async_runtime_plugin::{
    AppAsyncExt, AsyncRuntimePlugin, AsyncTaskError, WorldAsyncExt,
};
use core_plugin::{App, Event, World};

struct Double(u32);
impl Event for Double {}

async fn double(request: Double) -> Result<u32, AsyncTaskError> {
    Ok(request.0 * 2)
}

fn main() {
    let mut app = App::new();
    let done = Arc::new(AtomicBool::new(false));
    let received = Arc::clone(&done);
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_async_system(RuntimePlugin::UPDATE, double)
        .add_system(RuntimePlugin::UPDATE, move |world: &mut World| {
            for result in world.event_reader::<Result<u32, AsyncTaskError>>() {
                match result {
                    Ok(value) => assert_eq!(*value, 42),
                    Err(error) => eprintln!("{error}"),
                }
                received.store(true, Ordering::Release);
            }
        });
    app.world().send_async_event(Double(21));
    app.run_until(|| done.load(Ordering::Acquire));
}
```

每个请求类型注册一个 handler，发送入口使用 `send_async_event` 或 `send_await_event`。成功值和错误满足 `Send + Sync + 'static`，错误实现 `From<AsyncTaskError>`；业务错误可用自定义枚举或 `anyhow::Error` 表达。

## 选择任务入口

| 场景 | 注册与提交 | 调度行为 |
| --- | --- | --- |
| 可复用请求处理 | `add_async_system` + `send_async_event` | 在注册阶段读取请求并提交任务 |
| 一次性任务 | `send_async_closure(schedule, task)` | 通过目标阶段的闭包执行器提交 |
| 任务期间暂停后续帧 | `send_await_event` / `send_await_closure` | 实际分发时关阀，任务完成后开阀 |

闭包入口先安装 `ClosurePlugin`，再对目标 Schedule 调用 `add_closure_system`。闭包捕获拥有所有权的数据，形状为 `move || async move { ... }`。

## 公开对象与结果生命周期

- `AsyncRuntimePlugin` / `AsyncRuntimeHandle`：安装执行线程并持有其生命周期。
- `AppAsyncExt`：注册请求类型和可复用 handler。
- `WorldAsyncExt`：提交 Async 或 Await 任务。
- `AsyncEventHandler` / `AsyncTask`：适配 handler 和一次性闭包。
- `AsyncContext`：通过可选参数注入任务，发送进度或延迟事件。
- `AsyncTaskError`：将被监督 Future 的 panic 或取消转换为错误结果。

请求与响应各自占有事件节点。任务完成时填充 Pending 响应并唤醒 Runtime，后续事件帧将完整 `Result<T, E>` 提供给读取器。结果按完整 Rust 类型分流；需要请求关联时，把业务 ID 放入结果结构及业务错误中。

结果接收 System 持续注册，需要保留的数据由其写入资源或组件。Await 的等待发生在帧边界，当前帧继续执行。任务中的子任务由业务持有停止信号和句柄，等待其实际退出后返回任务结果；应用在业务清理完成后结束运行循环。

## 继续阅读

- [公开 API 接入测试](tests/public_api.rs)：`anyhow::Error` 与 `AsyncContext` 进度事件。
- [任务入口](src/request.rs)、[调度与结果监督](src/plugin.rs)、[执行线程](src/runtime.rs)。
- [设计与完整接口](DESIGN.md)。
