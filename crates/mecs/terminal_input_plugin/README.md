# TerminalInputPlugin

`TerminalInputPlugin` 将本地终端输入转换为类型化 ECS 事件。它支持显式选择 raw 或 cooked
模式，并处理终端尺寸、焦点、粘贴和鼠标事件；在有序关闭或 Resource 被丢弃时恢复终端状态。

```rust
use core_plugin::App;
use external_event_plugin::ExternalEventPlugin;
use terminal_input_plugin::{TerminalInputOptions, TerminalInputPlugin};

let mut app = App::new();
app.add_plugins(ExternalEventPlugin);
app.add_plugins(TerminalInputPlugin::with_options(
    TerminalInputOptions::raw()
        .with_bracketed_paste(true)
        .with_mouse_capture(true),
));
```

该 Plugin 不实现 `Default`；是否取得 stdin 所有权或启用 raw 模式，必须由组合根显式决定。
它不渲染 UI、不解释快捷键、不管理进程信号，也不启动伪终端子进程。同一进程只能有一个
活动会话持有 stdin；第二个会话会报告 `TerminalInputFailureKind::AlreadyInUse`。
