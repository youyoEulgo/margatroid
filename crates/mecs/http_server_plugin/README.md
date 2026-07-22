# http_server_plugin

`http_server_plugin` 将 Axum HTTP Server 接入 mecs App 生命周期，同时保留原生
Axum Router 和 Handler 的使用方式。

```rust
use app_runtime_plugin::AppRuntimePlugin;
use axum::{routing::get, Router};
use core_plugin::App;
use http_server_plugin::{HttpAppExt, HttpServerPlugin};

let mut app = App::new();
app.add_plugins(AppRuntimePlugin);
app.add_plugins(HttpServerPlugin::default());
app.add_http_routes(Router::new().route("/health", get(|| async { "ok" })));
```

Plugin 负责 listener、Tokio worker、请求限制和优雅停止，不定义业务路由。
