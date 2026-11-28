# workspace_core

`workspace_core` 提供 Workspace 组件、生命周期事件和 SQLite Agent 登记库。
项目内依赖为 mecs 的 `core_plugin`；数据库操作使用同步 API，等待操作收口使用异步通知。
领域身份和目录布局见 [Workspace](../README.md)，运行调度见 [workspace_plugin](../plugin/README.md)。

## 核心对象

| 对象 | 接入时的作用 |
| --- | --- |
| `WorkspaceDatabase` | 共享连接、操作受理门与登记提交快照 |
| `WorkspaceOperation` | 绑定特定连接与操作键的受理凭证，Drop 释放占用 |
| `WorkspaceAgent` | 持久 id、父节点、Driver 参数、当前及归档身份 |
| `WorkspaceRecordsSnapshot` | 同一 revision 的 metadata 与完整 Agent 列表 |
| `Workspace` / `WorkspaceState` | ECS 组件及 Running、Deleting、Error 状态 |
| `WorkspacePendingWork` | 启动加载及已受理创建工作的存续计数 |
| `WorkspaceUpdateContext` | 安装、状态变化和移除时调用的宿主发布回调 |

数据对象见 [types](src/types.rs)，事件与原始 oneshot 类型见 [events](src/events.rs)。
`WorkspaceConfig` 是独立 Resource，持有管理根路径。

## 从准备登记到提交

`create(path, metadata)` 要求全新数据库文件和已准备的 `agents/normal`、
`agents/temporary`、`agents/archived` 目录；`open(path)` 打开既有 v1 数据库并恢复归档意图。
以下函数接收已打开的数据库；完整创建 fixture 见 [数据库测试](tests/database.rs)。

```rust
use serde_json::json;
use workspace_core::{WorkspaceAgent, WorkspaceDatabase};

fn register(database: &WorkspaceDatabase)
    -> Result<WorkspaceAgent, Box<dyn std::error::Error>>
{
    let operation = database.try_operation("coder")?;
    let candidate = database.prepare_agent(
        &operation,
        "/demo".into(),
        Some("coder".into()),
        Some("Coder".into()),
        "/tools/driver/tool.toml".into(),
        json!({"node_name":"coder"}),
    )?;
    std::fs::create_dir(database.agent_directory(&candidate)?)?;
    let record = database.commit_agent(&operation, candidate)?;
    drop(operation);
    Ok(record)
}
```

该例针对 `/demo` 下新建的普通 Agent；实际 Driver 流程在 commit 前完成 Agent 文件准备，
并把 operation 保持到 StartAgent 回复及子任务绑定完成。
跨 worker 持有凭证时使用 `Arc<WorkspaceOperation>`，最后一个 Arc 释放操作键。
临时 Agent 使用 `temporary_operation_key()` 受理操作，并传入 `node_name=None`。
普通节点复用要求 Driver 路径与父节点匹配；冲突返回 `DriverMismatch` 或 `ParentMismatch`。

## 连接生命周期与提交观察

`WorkspaceDatabase::clone()` 共享连接与受理状态；`same_connection()` 判断共享来源。
同一操作键并发受理返回 `Busy`，凭证要求由当前数据库签发。
关闭顺序为 `close_admission()`、`wait_idle().await`、`close()`。
`close()` 在凭证存续时返回 `Busy`，成功关闭后向 watch 发布 `None`。
SQLite 关闭故障返回 `CloseFailed` 并保留连接供重试。

`subscribe_changes()` 提供确认快照，登记、归档与别名的实际提交变化推进 revision。
`rename_workspace_alias(String)` 修改 Workspace 展示名；
`rename_agent_alias(id, Option<String>)` 按持久 id 修改或清空 Agent 展示名。
`archive_agent(&operation, id)` 要求运行任务已经收口，具体恢复与错误阶段见 [database](src/database.rs)。

异步调用方把 SQLite 和目录操作交给 blocking worker 并等待结果。
[提交与关闭测试](tests/database.rs) 演示快照、凭证、归档恢复与共享连接行为。
`Workspace::set_state` 返回实际变化标志，宿主释放组件借用后通过
[WorkspaceUpdateContext](src/sync.rs) 发布变化。
`WorkspacePendingWork` 的 token 由运行调度方保留到装配事件处理完毕，详见 [pending](src/pending.rs)。
