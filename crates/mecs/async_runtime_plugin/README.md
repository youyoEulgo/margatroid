# AsyncRuntimePlugin

`AsyncRuntimePlugin` 为 mecs 提供专用异步线程，不把 Tokio 引入 `core_plugin`。

## Quick Start

```rust
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::{
    AppAsyncExt, AsyncRequest, AsyncRuntimePlugin, AsyncTaskError,
};
use core_plugin::App;

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

let mut app = App::new();
app.add_plugin(RuntimePlugin::default())
    .add_plugin(AsyncRuntimePlugin)
    .register_async_request::<String, LoadError>();

app.world().event_write().send_event(
    AsyncRequest::<String, LoadError>::new(|| async {
        Ok("done".to_string())
    }),
);
```

## Public API

- `AsyncRuntimePlugin`：启动并管理专用异步线程。
- `AsyncRequest<T, E>`：封装一次只能执行一次的异步闭包。
- `AsyncRequest::new`：普通请求，完成后唤醒 Runtime。
- `AsyncRequest::blocking`：阻止下一帧开始，完成后开阀。
- `AppAsyncExt::register_async_request`：注册到默认 `PRE_UPDATE` Schedule。
- `AppAsyncExt::register_async_request_in`：注册到指定 Schedule。
- `AsyncTaskError`：将任务 panic 或取消转换进开发者选择的错误类型 `E`。
- `AsyncRuntimeError`：报告异步基础设施的配置与运行错误。

每个请求独立 `tokio::spawn`。异步任务不借用 `World`；完成结果通过
`Result<T, E>` 事件返回主线程。请求 ID、Agent ID 和响应路由由开发者定义。
