# storage_core

`storage_core` 提供 Agent 存储的同步 API，以及 ECS 使用的组件和请求类型。
项目内依赖为 mecs 的 `core_plugin`；SQLite、JSON、TOML 与 watch 由通用库提供。
领域概念与目录布局见 [Storage](../README.md)，精确行为见 [DESIGN](../DESIGN.md)。

## 核心对象

| 对象 | 接入时的作用 |
| --- | --- |
| `AgentStorage` | `open_storage` 返回的组件句柄，克隆共享数据库状态 |
| `StorageOperation` / `StorageValue` | Block、配置和历史操作及其结果 |
| `MessageSelector` / `MessageSource` | 描述消息字段、数组范围，以及存储或字面量来源 |
| `RecordRange` / `RecordPage<T>` | 描述排他序号范围、升序记录和双向续读游标 |
| `RecordChanges` / `RecordWatermarks` | watch 接收器与消息、MCL、日志的已提交水位 |
| `StorageError` / `StorageErrorKind` | 结构化错误及调用方分支依据 |

对象定义见 [types](src/types.rs)，句柄访问方法见 [components](src/components.rs)。
`settings_path()` 返回实际设置路径，`data_dir()` 返回设置文件所在目录。
`same_database()` 判断共享来源，`is_open()` 读取当前开放状态。

## 从打开到关闭

以下示例使用本 crate 的测试依赖 `tempfile` 和 `serde_json`：

```rust
use serde_json::json;
use storage_core::{
    append_message_history, close_storage, open_storage,
    read_message_page, RecordRange,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let storage = open_storage(
        &directory.path().join("state.sqlite"),
        &directory.path().join("setting.toml"),
        "temperature = 0.5\n",
    )?;
    let messages = append_message_history(
        &storage, &[json!({"type":"user", "content":"hello"})], None,
    )?;
    let page = read_message_page(&storage, RecordRange {
        before_sequence: None,
        after_sequence: None,
        limit: 20,
    })?;
    assert_eq!(page.records.len(), messages.len());
    close_storage(&storage)?;
    Ok(())
}
```

全部文件和数据库 API 均为同步操作。异步调用方把句柄克隆移入 `spawn_blocking`，
等待 JoinHandle 和业务结果后继续生命周期；ECS 调度接入见 [storage_plugin](../plugin/README.md)。
关闭先取得共享管理锁，使全部克隆退休，再物理关闭连接并释放 TEMP。
`CloseFailed` 保留连接供显式关闭重试；退休句柄访问返回 `Closed`。
完整生命周期案例见 [close 测试](tests/close.rs)。

## 按任务查接口

- Block 创建、读取和替换：`execute_storage` 配合 `StorageOperation`，见 [选择器测试](tests/selectors.rs)。
- 快照与回档：`snapshot_block`、`read_block_snapshots`、`restore_block`，见 [快照测试](tests/snapshots.rs)。
- 消息追加：`append_message_history` 返回带真实 sequence 的消息，见 [消息实现](src/messages.rs)。
- 日志：`append_agent_log`、`read_agent_logs`，见 [日志测试](tests/agent_logs.rs)。
- 分页：`read_message_page`、`read_mcl_page`、`read_agent_log_page`，见 [分页测试](tests/pages.rs)。
- 提交观察：先 `subscribe_record_changes`，再读取基线与增量；通知允许合并，按水位追齐实际记录。

`RecordRange.limit` 要求正数，双界要求 `after_sequence < before_sequence`，
数值要求位于 SQLite 整数范围；范围错误返回 `InvalidRange`。
`CommitUnknown` 表达提交事实待确认，`CleanupFailed` 表达连接清理故障并退休共享状态。
设置操作按点分路径更新 TOML，具体错误与结构校验入口见 [lib](src/lib.rs)。
