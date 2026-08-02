# daemon_api_plugin

`daemon_api_plugin` 向基础设施 ServerPlugin 注册 Margatroid 产品路由，不持有 listener、HTTP
runtime 或进程关闭策略。当前只提供 health 与可选日志流；业务路由将在对应 Command/Result
API 落地后接入。

```rust
use app_runtime_plugin::AppRuntimePlugin;
use core_plugin::App;
use http_server_plugin::HttpServerPlugin;
use daemon_api_plugin::DaemonApiPlugin;

let mut app = App::new();
app.add_plugins(AppRuntimePlugin);
app.add_plugins(HttpServerPlugin::default());
app.add_plugins(DaemonApiPlugin::default());
```

可选日志端点将 `log_plugin::LogStream` 适配为带 bearer token 鉴权的 SSE。
