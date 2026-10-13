# MemoryPlugin

本 crate 只负责事务性追加历史，不保存 MCL 实时 state，不依赖 AgentPlugin，不转发世界事件。

`SqliteHistoryStore` 创建父目录并打开 SQLite 数据库，只初始化 `history_records` 表。历史以
`HistoryRecord { key, source, turn, kind, content, payload }` 保存，按插入序号读取。
`payload` 是 JSON Value；其业务结构由写入方定义，存储层不合成消息、工具 schema 或 token usage。

```rust
use memory_plugin::{AgentHistoryHandle, HistoryRecord, SqliteHistoryStore};
use serde_json::json;

let store = SqliteHistoryStore::open(":memory:")?;
let history = AgentHistoryHandle::new(store);
history.append_batch(vec![HistoryRecord {
    key: "turn-1/user-1".into(),
    source: "driver".into(),
    turn: Some("turn-1".into()),
    kind: "message.user".into(),
    content: "hello".into(),
    payload: json!({"message": {"role": "user", "content": "hello"}}),
}])?;
assert_eq!(history.list()?.len(), 1);
# Ok::<(), memory_plugin::HistoryError>(())
```

`append_batch` 全写或全不写。空 key 返回 `InvalidRecord`；已存在 key 的全部字段相同则幂等，
不同则返回 `KeyConflict` 并回滚整批。同批重复 key 遵循同一规则。JSON 按 Value 比较，
对象的序列化字段顺序不会影响幂等性。重试不会产生新序号，也不会重排已有历史。

`SqliteHistoryStore` 可 Clone，内部共享 `Arc<Mutex<Connection>>`，也可通过
`AgentHistoryHandle` 在多个线程间共享。读取和数据库错误使用 `HistoryError` 返回。

可选 `MemoryPlugin::new()` 只安装 `MemoryPluginInstalled` 标记，无需 RuntimePlugin 或 schedule。
Agent 负责创建自己的历史 store；写入方同步调用 handle 并处理错误，不依赖异步转发系统。
展示层使用 `list()` 的完整记录快照，按 `kind/content/payload/turn` 转换 DTO。

不再导出 `AgentMemory`、`HistoryMessage`、实时 state API、旧错误与写入事件。
本实现不迁移旧 `history_messages`、`setting` 或 `state` 表，也不删除已有数据库中的旧表。
