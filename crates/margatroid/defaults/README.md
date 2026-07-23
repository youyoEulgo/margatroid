# margatroid_defaults

该 crate 提供 Margatroid V3 daemon 的最小默认 Plugin 组合：日志、运行循环、外部事件、
进程信号、异步 runtime、HTTP、产品路由和 daemon 生命周期。

```rust
use core_plugin::App;
use margatroid_defaults::MargatroidDaemonPlugins;

let mut app = App::new();
app.add_plugins(MargatroidDaemonPlugins::default());
```

尚未实现稳定 API 的资源库、Workspace、LLM、Sandbox、Skill 和 Workflow 不会以占位
Plugin 的形式预装。它们在各自路线图阶段完成后再加入产品组合。
