# storage_plugin

`storage_plugin` 把 ECS 的 `StorageRequest` 交给 blocking worker 执行，
并通过请求携带的原始 oneshot 返回 `Result<StorageValue, StorageError>`。
领域概念见 [Storage](../README.md)，同步文件与数据库接口见 [storage_core](../core/README.md)。

## 接入对象

| 对象 | 作用 |
| --- | --- |
| `StoragePlugin` | 安装请求与异步结果处理 System |
| `AgentStorage` | 挂在请求 owner Entity 上的已连接句柄 |
| `StorageRequest` | 携带 owner、operation、cancel 和 reply 的输入事件 |
| `StorageReply` | `Arc<Mutex<Option<oneshot::Sender<...>>>>`，用于领取并完成原请求 |
| `StorageOperation` / `StorageValue` | 来自 core 的操作与返回值 |

插件公开入口见 [lib](src/lib.rs)，请求结构见 [core events](../core/src/events.rs)。
插件依赖 storage core，以及 mecs 的运行时、异步和闭包基础设施。

## 安装到 App

运行时和闭包系统先安装，随后安装异步运行时与存储插件：

```rust
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin};
use core_plugin::App;
use storage_plugin::StoragePlugin;

let mut app = App::new();
app.add_plugin(RuntimePlugin::default())
    .add_plugin(ClosurePlugin)
    .add_closure_system(RuntimePlugin::UPDATE)
    .add_plugin(AsyncRuntimePlugin)
    .add_plugin(StoragePlugin::new());
```

`StoragePlugin::new()` 使用 `RuntimePlugin::UPDATE`。
`with_schedule(name)` 将存储 System 与异步完成闭包放到指定 schedule；
调用方同时为该 schedule 配置闭包系统，并持续驱动 App。
每个 App 安装一次插件，安装时要求 `AsyncRuntimeHandle` 已存在。

## 一次请求如何完成

1. 在 blocking worker 调用 `open_storage`，把返回的 `AgentStorage` 挂到 owner Entity。
2. 创建 oneshot，把 sender 包装进 `StorageReply`，随 `StorageRequest` 发给 World。
3. System 领取 sender，按 Entity 代际取得组件克隆，并提交异步闭包。
4. 异步闭包在 `spawn_blocking` 中调用 `execute_storage`，等待真实 worker 返回。
5. 完成闭包把业务值或错误交回同一 sender，调用方接收原 oneshot 结果。

完整的组件挂载、`CreateBlock` 请求、tick 驱动及结果接收示例见
[plugin 测试](tests/plugin.rs) 的 `app`、`request`、`wait` 与首个测试。
请求消费和结果顺序见 [system](src/system.rs)，worker 与回复持有关系见 [handler](src/handler.rs)。

## 取消、错误与生命周期

owner 查找失败返回 `OwnerMissing`；执行前观察到取消返回 `Cancelled`。
操作已经提交时，以实际提交结果完成回复。
worker panic 或结果丢失返回 `CommitUnknown`，任务提交故障返回 `WorkerFailed`。
异步闭包清理期间，worker guard 等待真实 worker 退出并交付结果。
对应案例见 [plugin 测试](tests/plugin.rs) 中的取消、写入错误和 shutdown 测试。

Agent 的创建、存储打开和显式关闭由所属生命周期组织。
关闭流程先等待请求、日志和文件 worker 收口，再在 blocking worker 调用 core 的 `close_storage`。
日志、分页和快照调用使用 [core API](../core/README.md)，
各调用方持有句柄并等待其实际执行结果。
