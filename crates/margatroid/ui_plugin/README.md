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

## 用户图片

- 将图片拖入编辑器，或点击左下角加号 →「上传图片 / Upload image」选择图片。两者共用校验与导入逻辑，可多次添加、逐张删除，也可仅发图片。
- 预览独占可横向滚动的一行，正文另起一行。支持 PNG/JPEG/GIF/WebP；每条最多 8 张、每张 5 MiB、合计 20 MiB（解码字节），拒绝伪装格式和 SVG。
- 发送沿用 MCL user 消息，图片为 `images: [{media_type, data}]`（无前缀 base64）；文件名仅用于界面。失败保留草稿，切换 agent/workspace 清空草稿并隔离迟到异步结果。
- 对话图片仅来自后端权威历史，显示在独立图片行，不将 base64 展示为活动或元数据文字。
- 发送期间编辑器显示当前等待的是注入回执还是唤醒回执；Activity 的 `ui.send` 记录阶段与图片数量，不记录正文、文件名或 base64。注入成功不等于模型已开始运行，必须继续收到 wake 回执。
- 回执被拒绝、连接关闭或现有 120 秒回执超时均会结束发送等待并保留草稿；超时不代表后台命令已取消，重试前应核对历史，避免重复注入。
- UI 回归测试以超过 1 MiB 的序列化图片负载走 WebSocket `onmessage` 与协议解析路径，验证回执前不唤醒、`request.failed` 拒绝后可重试及历史图片投影。该测试仍使用假 socket，不替代 daemon 的真实 WebSocket 大消息集成测试或浏览器解码/响应性测试。

## 回合中止交互（已实现）

- 发送与中止以同步状态为准：`idle` 允许发送；`working` 只允许中止；`creating/stopping` 不允许发送。
- 中止立即清空 `turn`，但收口期间仍为 `working`，不能提前开放新消息；只有 Driver 的 `finish_turn` 完成并同步 `idle` 后恢复发送。
- 中止通过独立控制请求发送，不进入 MCL FIFO；请求受理回执不是清理完成通知。
- 点击中止立即冻结当前实例/回合的流式显示，无需等待控制回执；已知请求 ID 的迟到分片跨回合继续忽略，未知请求在权威新回合前也不追加。HTTP 取消仍异步进行，不伪造 Idle 或清理完成。
- 历史 assistant 无论 `stop_reason` 是否为 `cancelled`，均按普通消息显示已有思考和正文；不加红色、取消标签或空内容兜底。权威历史仍可对齐服务端接受的部分输出。
- 关联中止请求被拒绝时显示真实错误，并仅对仍属于同一实例/回合的临时冻结解除；迟到拒绝不能改变新回合。
- 取消 assistant 允许记录历史但不进入模型上下文；取消工具结果仍保存历史与上下文，保留调用关联。
- 真正清理失败仍显示错误，不伪装成取消成功或可继续状态。

停止按钮发送 agent.turn.abort，并按请求 ID、workspace、agent 和 turn_id 核对回执；受理成功后保持禁用，等待状态同步确认 Idle。

中止执行顺序：AgentPlugin 的 abort_agent_turn_system 固定运行于 RuntimePlugin::PRE_UPDATE，先于 UPDATE 的 MCL 与推理/工具 admission；UI 仅发送控制请求并等待权威状态同步，不承担清理完成判定。
