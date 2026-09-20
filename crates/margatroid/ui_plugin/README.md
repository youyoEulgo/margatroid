# UiPlugin

`UiPlugin` 把 `ui/dist` 在编译期嵌入 daemon 二进制，并在同一端口上提供界面。

```rust
use core_plugin::App;
use server_plugin::AppServerExt;
use ui_plugin::UiPlugin;

let mut app = App::new();
// ... 安装其它插件
app.add_plugin(UiPlugin::default());
app.add_http_routes(UiPlugin::router());
```

界面与 WebSocket 后端同源：

```text
http://127.0.0.1:3939/      界面
ws://127.0.0.1:3939/ws      WebSocket
```

`ui/dist` 不入库，构建前先执行 `scripts/build-ui.sh`。嵌入发生在编译期，因此改动界面后需要
重新构建界面并重新编译 daemon。
