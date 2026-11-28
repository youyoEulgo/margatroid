# workspace_plugin

`workspace_plugin` 把 Workspace 的加载、创建、装配、Agent 启动、归档和删除接入 ECS。
它协调 workspace、agent、tool 的 core 能力与 mecs 运行设施。
领域概念见 [Workspace](../README.md)，登记库接入见 [workspace_core](../core/README.md)。

## 接入对象

| 对象 | 在插件流程中的作用 |
| --- | --- |
| `WorkspacePlugin { root }` | 配置管理根并安装 STARTUP / UPDATE Systems |
| `CreateWorkspace` / `StartWorkspace` | 准备目录与连接，随后装配运行实体 |
| `StartWorkspaceAgent<R, E>` | New / Existing 两种 Driver 启动请求 |
| `ArchiveAgent` / `DeleteWorkspace` | 发起归档或 Workspace 生命周期清理 |
| `WorkspacePendingWork` | 跟踪加载与创建，直到 World 完成装配 |
| `WorkspaceUpdateContext` | 宿主注入的生命周期发布回调 |

事件定义见 [core events](../core/src/events.rs)，安装入口见 [lib](src/lib.rs)。

## 安装与创建请求

`root` 要求绝对路径，每个 App 安装一次插件，安装时要求异步运行时已就绪。
下例中的项目路径由调用方准备为实际目录，管理根与项目目录各自独立：

```rust
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin};
use core_plugin::App;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;
use workspace_core::CreateWorkspace;
use workspace_plugin::WorkspacePlugin;

let mut app = App::new();
app.add_plugin(RuntimePlugin::default())
    .add_plugin(ClosurePlugin)
    .add_closure_system(RuntimePlugin::UPDATE)
    .add_plugin(AsyncRuntimePlugin)
    .add_plugin(WorkspacePlugin { root: "/srv/margatroid/workspaces".into() });
let (sender, receiver) = oneshot::channel();
app.world().emit_event(CreateWorkspace {
    address: "/demo".into(),
    alias: "Demo".into(),
    project_root: "/srv/projects/demo".into(),
    reply: Arc::new(Mutex::new(Some(sender))),
});
```

宿主持续 tick App，调用方通过 `receiver` 接收 `Result<Entity, WorkspaceError>`。
完整临时目录设置、tick 驱动和回复接收见 [生命周期测试](tests/lifecycle.rs)
中的 `app`、`receive` 与 `create_prepares_database_then_starts_entity_and_replies_original_once`。
STARTUP 扫描管理根，在 blocking worker 打开登记库，再通过 `StartWorkspace` 挂载实体。
加载错误经 tracing 输出；`WorkspacePendingWork.is_idle()` 同时覆盖启动加载与已受理创建。

## 启动 Driver 与归档 Agent

`StartWorkspaceAgent<ToolExecuteResponse, ToolError>::New` 接受绝对工具路径与 JSON object 参数，
将原始 reply 转交 `ToolExecuteRequest::Path`。工具运行由宿主安装的工具能力承接。
`Existing` 同步查询登记，使用原父节点实体与 Driver 路径，按当前 node_name、alias 整理参数；
Driver 自定义参数沿用已登记值。实际登记提交由 Driver 创建注入流程负责。

`ArchiveAgent` 取得节点操作凭证并定位所属父任务，等待 `task.finish()` 后，
在 blocking worker 调用 `archive_agent`，把结果交回原始请求。
运行身份、父节点和目录冲突通过结构化错误返回。
启动与归档案例集中在 [生命周期测试](tests/lifecycle.rs)。

## 删除与变化发布

删除将 Workspace 置为 Deleting，并关闭数据库与 ToolTasks 的受理门。
异步清理等待各任务真实完成和数据库凭证释放；完成处理再校验管理目录、
调用 `database.close()`、移除 Entity，最后删除管理目录并完成 reply。
清理错误把仍存续的 Workspace 置为 Error；目录删除错误携带 `entity_removed` 与实际目录信息。

宿主可预先注入 `WorkspaceUpdateContext`，接收 Installed、Changed、Closing、Removed 回调。
数据库记录变化由 core 的 watch 提供，生命周期回调在组件借用释放后发布。
各 System 的事件顺序见 [system](src/system.rs)，完整清理路径见 [handler](src/handler.rs)。
