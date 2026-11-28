# agent_plugin

`agent_plugin` 把 Agent 生命周期接入 mecs 调度和异步运行时。
它使用 Agent、Workspace、工具、推理、MCL、存储等领域 core 完成创建和关闭编排。

## 核心入口

- `AgentPlugin::new(sync)` 接收装配层提供的 `AgentUpdateContext`。
- `with_schedule` 选择调度，默认使用 `RuntimePlugin::UPDATE`。
- `StartAgent<AgentStorage>` 在当前调度帧创建完整 Agent 实体，回复 `AgentHandle`。
- `StopAgent` 找到目标的实际创建任务，异步等待该任务及其绑定 Agent 清理完成。

## 装配插件

应用先安装运行时和闭包系统，再安装 AgentPlugin：

```rust
use agent_core::AgentUpdateContext;
use agent_plugin::AgentPlugin;
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin};
use core_plugin::App;

fn main() {
    let mut app = App::new();
    let updates = AgentUpdateContext::new(|_, _, _| vec![], |_, _| {});
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(ClosurePlugin)
        .add_closure_system(RuntimePlugin::UPDATE)
        .add_plugin(AgentPlugin::new(updates));
}
```

示例使用空观察回调展示插件安装。生产装配中的回调负责公共信息和 UI 等来源观察，
接入位置见 [compose](../../compose/src/lib.rs)。

## 提交创建请求

创建前准备 Running Workspace、同 Workspace 的父实体、父实体上已登记且可受理的
ToolTask，以及已打开的 AgentStorage。`StartAgent` 中的 `address` 使用父地址加叶名，
`project_root` 与 `ui_path` 使用绝对路径，`persistence` 传入存储句柄。
通过 `WorldEventExt::send_event` 发送请求并接收 oneshot，具体构造见
[生命周期测试的 Fixture](tests/lifecycle.rs)。

创建成功后实体处于 Starting，调用方经 ready 流程进入 Running。
插件将 handle 绑定到创建任务，并安装 MclQueue、Tools、ToolTasks、ActiveInference 和存储。
关闭先切换 Closing，再等待各实际活动及存储完成清理，最终移除实体。
调度中按添加顺序执行 stop 系统和 start 系统。

## 查阅与验证

- [插件入口](src/lib.rs)：安装条件与系统注册。
- [请求处理](src/handler.rs)：创建校验、任务绑定、关闭和失败收口。
- [生命周期测试](tests/lifecycle.rs)：完整接入、来源 worker 等待与关闭失败路径。
- [领域导航](../README.md) 与 [设计](../DESIGN.md)。

运行 `cargo test -p agent_plugin` 验证编排行为。
