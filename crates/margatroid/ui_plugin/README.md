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

## 回合中止交互设计（待实现）

- 发送与中止以同步状态为准：`idle` 允许发送；`working` 只允许中止；`creating/stopping` 不允许发送。
- 中止立即清空 `turn`，但收口期间仍为 `working`，不能提前开放新消息；只有 Driver 的 `finish_turn` 完成并同步 `idle` 后恢复发送。
- 中止通过独立控制请求发送，不进入 MCL FIFO；请求受理回执不是清理完成通知。
- 历史 assistant 的 `stop_reason: "cancelled"` 使用红色消息展示，标准英文正文为 `Turn aborted.`。不按正文猜测取消，也不改写成 Error 类型。
- 取消 assistant 允许记录历史但不进入模型上下文；取消工具结果仍保存历史与上下文，保留调用关联。
- 真正清理失败仍显示错误，不伪装成取消成功或可继续状态。

以上为已确认设计，尚未实现完整中止入口；当前行为以源码为准。
