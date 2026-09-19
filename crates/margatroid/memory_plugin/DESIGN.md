# MemoryPlugin 设计

MemoryPlugin 为每个 Agent 提供独占的 SQLite 存储。历史消息和 MCL state 是两种不同语义：

- `history_messages` 是追加式对话时间线，由 `history_append` 显式写入；
- `state` 是由 MCL driver 声明的通用持久化存储，按 key 保存完整 Block JSON；
- state 的字段形状由 driver 的 `CREATE BLOCK` 决定，MemoryPlugin 不预设业务字段。

# lib

## 类型

```text
MemoryPlugin：Agent 记忆插件
    schedule: String--持久化 System 所属 Schedule
    new() -> Self
    with_schedule(mut self, schedule: impl Into<String>) -> Self
    build(self, app: &mut App)
        安装历史消息与显式记录写入系统；state 由 MCL direct operation 通过 AgentMemoryHandle 同步读写

MemoryPluginInstalled：插件安装标记

AgentMemory：单 Agent SQLite 存储
    open(path: impl Into<PathBuf>) -> Result<Self, MemoryError>
        创建父目录、打开数据库并初始化 history_messages 与 state 表
    path(&self) -> &Path
    history_messages(&self) -> Result<Vec<HistoryMessage>, MemoryError>
        按 sequence 读取展示历史
    impl AgentMemoryStore for AgentMemory
        append_history：追加一条消息历史
        set_state：按 <name> key 写入完整 block JSON
        state_value：读取完整 state JSON blob
        append_record：追加显式 MCL timeline record
```

# system

## 函数

```text
sync_history_messages_system(world: &mut World)
    处理 AgentHistoryMessageWriteRequested，失败时发送 AgentMemoryWriteFailed

sync_history_records_system(world: &mut World)
    处理 AgentHistoryRecordWriteRequested，失败时发送 AgentMemoryWriteFailed

```

MCL state 的读取在 MCL direct operation 中通过 Agent.memory 同步完成；state 的写入同样通过
AgentMemoryHandle 完成，不再经过 realtime 专用读写事件。

# handler

## 函数

```text
handle_history_message_write(world, event)
    校验 Agent.memory 后追加历史消息

handle_history_record_write(world, event)
    校验 Agent.memory 后追加显式 timeline record
```

# events

MemoryPlugin 只处理历史消息与显式记录事件。state 通过 AgentMemoryStore 的通用 blob API 同步读写，不定义业务专用事件。

# types

## SQLite schema

```sql
CREATE TABLE history_messages (...);
CREATE TABLE state (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at_ms INTEGER NOT NULL
);
```

`state` 只承载一种通用数据：

```text
key = <state-name>
value = 完整 Block JSON
```

打开旧数据库时，如果存在旧 `setting` 表，只迁移其中 `mcl.state/<name>` 行为裸 `<name>` key；旧 RESOURCE 配置行被丢弃，迁移后删除旧表。

state 的读取规则由 MCL 定义：

```text
state 不存在：目标 block 保留 driver 默认值
state 存在但缺少字段：该字段保留默认值
state 存在且字段为空数组：覆盖默认值为空数组
state 存在且字段有值：覆盖默认值
```

未知字段被忽略，当前 block 新增字段使用 driver 默认值。

# 逻辑

```text
Agent 启动
    -> AgentMemory::open
    -> 创建 AgentMemoryHandle
    -> Base Lua CREATE BLOCK realtime_state
    -> LOAD STATE realtime INTO realtime_state
    -> 将 state 字段回填 msg
    -> BIND realtime_state TO STATE realtime

普通消息写入
    -> Base Lua EMIT EFFECT history_append FROM ?
    -> AgentHistoryMessageWriteRequested
    -> sync_history_messages_system
    -> history_messages 追加一行

state 更新
    -> Base Lua INJECT 数据到已绑定 block
    -> MCL 序列化完整 block
    -> AgentMemoryHandle::set_state
    -> state 表 upsert
```

# 边界

MemoryPlugin 负责通用 SQLite 存储，不知道 `state` 中具体保存的是 sandbox、工具可见性还是实时上下文。
Base Lua 负责 block 形状、默认值、加载顺序以及何时把消息上下文同步到 `realtime_state`。
`history_messages` 和 state 不互相恢复，也不自动将 state 记录到 timeline。
