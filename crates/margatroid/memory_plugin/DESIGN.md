# MemoryPlugin 设计

## 边界

memory_plugin 是只追加的事务历史存储模块。它依赖 margatroid_types 的历史契约，不依赖
agent_plugin，不保存实时上下文、MCL state、设置或资源状态，不负责消息领域转换。

公开导出 `SqliteHistoryStore`、`AgentHistoryStore`、`AgentHistoryHandle`、`HistoryRecord`、
`HistoryError`。可选 `MemoryPlugin` 仅安装资源标记，不注册系统、事件或 schedule。

## API

```text
SqliteHistoryStore: Clone + Send + Sync
    open(path: impl AsRef<Path>) -> Result<Self, HistoryError>
        创建父目录、打开数据库、设置 5 秒忙等待并初始化历史表
    impl AgentHistoryStore
        append_batch(Vec<HistoryRecord>) -> Result<(), HistoryError>
        list() -> Result<Vec<HistoryRecord>, HistoryError>

MemoryPlugin: Default
    new() -> Self
    impl Plugin
        build() 插入 MemoryPluginInstalled，重复安装报错
```

## SQLite schema

```sql
CREATE TABLE IF NOT EXISTS history_records (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    key TEXT NOT NULL UNIQUE CHECK(length(key) > 0),
    source TEXT NOT NULL,
    turn TEXT,
    kind TEXT NOT NULL,
    content TEXT NOT NULL,
    payload TEXT NOT NULL
);
```

序号仅用于稳定排序，不是领域记录字段。`list()` 按 sequence ASC 返回完整记录快照。
`turn` 保留可选字符串，payload 以 JSON 编码存储并在读取时解码为 Value。
时间戳、message、tool schema、usage 等领域元数据如需持久化，由写入方显式放入 payload。
新数据库不创建 state 表；打开旧数据库不会迁移或删除既有表。

## 原子写入与幂等

先检查全部 key 非空；空批成功。非空批在 `Arc<Mutex<Connection>>` 下开启 Immediate 事务。
逐条按 key 查询已存在记录（包括当前事务刚写入的记录），比较完整 HistoryRecord：

- 相同记录跳过，不生成新序号；JSON 使用 Value 比较而非字符串比较。
- 不同记录返回 KeyConflict，事务回滚全部新增行。
- 无已有 key 才执行 INSERT，保存所有字段。
- 全部成功才 commit；SQL 或编码错误返回 WriteFailed 并回滚。

Immediate 事务防止不同连接在检查 key 与 INSERT 之间交错写入；忙等待为并发连接提供等待窗口。
Clone 共享同一连接和互斥锁，AgentHistoryHandle 可将存储抽象跨线程传递。

## 替代旧接口

旧 AgentMemory/history_messages/set_state/state_value、写入事件、转发系统与 MemoryError 已移除。
Agent 创建 SqliteHistoryStore 并保存 AgentHistoryHandle，效果处理直接同步调用 append_batch，
根据返回结果决定是否继续领域状态提交。展示层读取 handle.list() 并在领域或 DTO 层转换，
不从实时状态恢复或拼接历史。

## 测试

内置测试覆盖重开持久化、JSON 和 turn 保存、表边界、同批及跨批幂等、JSON 对象顺序、
完整字段冲突、整批回滚、数据库失败、空 key、稳定序号和线程共享。
公开集成测试覆盖无 Agent/Runtime 的外部调用、trait handle 的原子冲突语义与纯历史表。
