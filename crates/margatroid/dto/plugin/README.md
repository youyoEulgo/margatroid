# dto_plugin

`dto_plugin` 为应用安装WebSocket入口、Workspace文件读取和状态同步。它连接 [protocol](../../protocol/README.md) 中的消息类型与各领域的执行能力。

## 接入应用

完整后端通过 [compose::build_app](../../compose/README.md) 装配本插件。自定义装配时，先准备运行调度、异步运行时、World闭包、Axum服务、配置及客户端集合，再安装领域插件和 `DtoPlugin::new()`。

领域写入使用同一批同步绑定。装配方可先创建绑定，再把各字段交给对应领域：

```rust
use client_core::Clients;
use dto_plugin::sync_bindings;

let clients = Clients::default();
let bindings = sync_bindings(clients.clone());
let agent_updates = bindings.agent;
let workspace_updates = bindings.workspace;
```

`DtoSyncBindings` 还提供Workspace绑定、任务观察绑定、工具观察工厂、模型和沙箱观察工厂。[装配代码](../../compose/src/handler.rs)展示了这些值的完整安装顺序。

## 入口与职责

`DtoPlugin` 安装 `/ws` 和Workspace文件路由，将普通请求接入UPDATE调度，将回合中止控制接入PRE_UPDATE调度。WebSocket处理函数负责连接读写，路由System负责请求受理，异步执行等待领域的原回复。

| 接口 | 用途 |
| --- | --- |
| `DtoPlugin::new` | 创建HTTP与WebSocket接入插件 |
| `sync_bindings` | 为同一Clients集合创建领域同步能力 |
| `agent_sync_context` | 创建Agent状态更新上下文 |
| `workspace_sync_binding` | 关联Workspace更新与登记快照 |
| `task_sync_binding` | 为ToolTasks集合安装同步观察 |
| `resource_sync_bindings` | 创建工具集合的观察工厂 |

任务观察通过 `TaskSyncInstaller` 交给新创建的任务集合。工具、模型和沙箱观察工厂在实际集合或配置来源出现时建立对应绑定。

## 状态、记录与来源

状态投递使用来源版本和当前快照，同一来源的连续更新可以合并。持久消息、MCL与日志读取Agent存储的提交水位，按实际序号交付新增记录。Driver UI和模型配置有各自的文件监控。

来源关闭会撤销投递能力，取消监控并等待在途读取完成。连接关闭从共享Clients集合移除注册身份。领域中已受理的任务继续沿各自的监督流程完成。

## 查阅实现

- [插件安装与公开绑定](src/lib.rs)
- [请求受理](src/system.rs)、[网络处理](src/handler.rs)
- [状态探针](src/probes.rs)、[Workspace来源](src/workspaces.rs)
- [记录同步](src/records.rs)、[UI资源读取](src/ui.rs)
- [领域概览](../README.md)、[设计参考](../DESIGN.md)
- [请求、查询与同步测试](../../compose/tests/dto_websocket.rs)
