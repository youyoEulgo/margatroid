# Storage

Storage 管理单个 Agent 的消息历史、MCL 审计、运行日志、消息 Block 和 TOML 设置。
首次接入可先阅读 [storage_core](core/README.md) 的同步调用示例；
使用 ECS 请求调度时，再阅读 [storage_plugin](plugin/README.md)。
结构、持有关系和函数行为见 [DESIGN](DESIGN.md)。

## 先认识数据与句柄

| 概念 | 用途 | 入口 |
| --- | --- | --- |
| AgentStorage | Agent Entity 上的共享存储句柄 | [组件](core/src/components.rs) |
| 历史 | 消息与 MCL 记录共用真实递增 sequence | [历史操作](core/src/lib.rs) |
| Agent 日志 | 使用独立 sequence 的诊断记录 | [日志](core/src/logs.rs) |
| 消息 Block | 按字段组织消息数组，支持选择、替换和快照 | [Block](core/src/blocks.rs) |
| 设置 | 逐次读取和原子替换 TOML 文件 | [配置操作](core/src/lib.rs) |
| 提交水位 | 观察消息、MCL、日志的已提交高水位 | [分页与水位](core/src/pages.rs) |

持久 Agent 使用以下布局；路径由创建 Agent 的调用方准备并传入：

```text
<Agent目录>/
  state.sqlite
  setting.toml
  state.sqlite.blocks/<Block名>/
    timeline.sqlite
    snapshot_<id>.sqlite
```

管理库保存历史、日志和持久 Block 映射，Block 正文保存在各自的 SQLite 文件。
TEMP Block 以运行 instance 与 Block 名为键保存在内存，快照同样采用内存副本。
`DropTemporaryBlocks` 负责释放指定 instance 的 TEMP 集合。

## 一次存储调用的路径

1. 创建方在 blocking worker 调用 `open_storage`，获得已连接的 `AgentStorage`。
2. Agent 装配流程把该句柄挂到 Entity；业务方选择同步 core 函数或 `StorageRequest`。
3. core 取得共享管理连接锁，执行对应数据库、Block 或设置操作并返回结果。
4. 历史或日志确认提交后更新 watch 水位，监控方按真实 sequence 分页追齐记录。
5. Agent 生命周期等待任务、记录写入和文件 worker 收口，再显式调用 `close_storage`。

同一开放管理库的重复打开与句柄克隆共享连接、TEMP 集合和受理锁。
持久 Block 连接在该锁内按操作打开并释放；设置使用额外的文件锁协调读改写。
关闭使全部共享句柄退休；成功后再次打开文件会建立独立共享状态。

## 选择合适的接口

历史追加使用 `append_message_history`，返回带真实 sequence 的内部消息。
Block 数组位置使用 `MessageSelection`，历史关联使用消息自身的 sequence。
消息与 MCL 共用时间线，Agent 日志水位按其独立序列解释。
分页使用排他 before/after 边界和正数 limit，结果按 sequence 升序返回。

`snapshot_block` 完整复制工作 timeline；`restore_block` 先归档当前 timeline，再恢复目标快照。
快照上游表达分支关系，快照 ID 表达分配顺序；恢复范围为所选 Block。
恢复意图与发布记录负责跨文件操作的中断恢复，详见 [快照实现](core/src/blocks.rs)。

设置路径、schema 和消息形状按 core 契约校验，调用方通过 `StorageError.kind` 选择处理方式。
管理库要求 schema v5，持久 Block 要求 schema v2，结构冲突返回 `SchemaMismatch`。
完整接入与边界案例可从 [core 测试](core/tests/storage.rs)、
[快照测试](core/tests/snapshots.rs) 和 [插件请求测试](plugin/tests/plugin.rs) 进入。
