# MemoryPlugin

本 crate 实现真实 SQLite Agent database：Block 与只追加 history 共用同一文件、connection manager，分表存放。不提供 RAM Block backend、自动同步或世界事件转发，不依赖 AgentPlugin/MCL plugin，不加载资源或激活沙箱。

## 数据库入口

`SqliteAgentDatabase::open(path) -> Result<Self, BlockStorageError>` 同步创建父目录、打开数据库、设置 5 秒 busy timeout，初始化并验证 `history_records`、`block_definitions`、`block_fields`。调用方必须把 open 放在 blocking worker；该方法本身不是 async。打开不清空已有数据或删除无关旧表。

```rust
use memory_plugin::SqliteAgentDatabase;
use margatroid_types::block_storage::AgentDatabaseHandle;

let store = SqliteAgentDatabase::open(":memory:")?;
let database = AgentDatabaseHandle::new(store);
let history = database.history();
let storage = database.block_storage();
# Ok::<(), margatroid_types::block_storage::BlockStorageError>(())
```

`SqliteAgentDatabase` 实现 `AgentHistoryStore` 和阻塞 `AgentBlockStorage`，符合共享 `AgentDatabaseStore` blanket contract。Clone 和两种 handle 视图共享 `Arc<Mutex<Connection>>` 及连接 retired 标记。`AgentBlockStorageHandle::execute(request).await` 通过 blocking worker 调用存储并等待真实 join；调用方负责 Other 活动登记和生命周期收口，不能丢弃 future 后假称工作完成。

## Block

- CREATE 使用 `BlockSchema { name, fields, temporary }`，返回 `BlockOpened { block_id, created, temporary }`。首次创建为 true；同 namespace 同 schema 为 false，不清空字段。schema 或 lifecycle 冲突不静默重建。
- Persistent namespace 为稳定 Agent ResourceID 与 block 名。TEMP 也是真实 SQLite 主数据库行，额外编码完整 Driver instance（Entity index/generation、start_request_id）；不是 SQLite TEMP 表或 RAM 内容。
- 启动方必须使 start_request_id 包含跨进程唯一的 startup/session nonce。重复完整 instance 值会打开原 TEMP；数据库不为每次 open 添加随机 nonce，以保留同实例幂等。
- 同 Agent/block 名的 lifecycle 在 SQLite metadata 中全局保留。旧 TEMP 行不自动删除、不同新 instance 不可见，但仍阻止同名 Persistent CREATE；显式清理/迁移不在当前 API 内。没有 RAM definition 映射或 Block cache。
- 每个 field 一行 typed array JSON。MESSAGE 保存完整消息，包括 System、递归 Inject、usage、工具调用关联；TOOL/SANDBOX 只存 canonical ResourceID，不存脚本、schema 或 policy 快照。资源 ID 的当前导入解析由调用方完成。
- GET 的所有 expressions/selectors 在同一 Deferred read transaction 中读取。INJECT 在同一 Immediate write transaction 中读取全部来源、目标长度、检查类型和 len+1 范围规则，再一次 replace；self-source 先读完全部来源再写。
- typed 空值保留类型；只有显式 `BlockExpression::Empty` 注入可依据目标类型构造空数组，不把目标类型用作任意来源转换。

open 校验列类型、NOT NULL、主键、字段外键及非 partial UNIQUE。坏 schema 返回 `SchemaMismatch`，不重置已有记录。

## 历史兼容 API

`SqliteHistoryStore::open` 仍只初始化 `history_records`，供原独立历史调用方使用。Agent 统一入口应使用 `SqliteAgentDatabase` 及其 history 视图，而不是再打开第二个独立 history store。

历史以 `HistoryRecord { key, source, turn, kind, content, payload }` 保存，按插入序号读取。`payload` 为 JSON Value，业务结构由写入方定义；存储层不合成消息、工具 schema 或 token usage。

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

`append_batch` 在 Immediate 事务中处理整批。空 key 返回 `InvalidRecord`；同 key 完整字段相同则幂等，不同返回 `KeyConflict` 并确认 rollback。同批重复 key 遵循同一规则；JSON 按 Value 比较，不受对象序列化顺序影响，重试不生成新序号。

Block 与 history 不隐式联动：GET/INJECT 不追加消息，history append 不修改 Block；不同指令没有跨事务原子保证。同步 history API 保留，ECS/MCL 使用时应由调用方调度 blocking worker。

## 提交、取消与清理事实

事务 body 或 commit 失败后显式 ROLLBACK 并检查 autocommit，不依赖忽略 rollback 错误的 Transaction Drop。提交前取消只有在事务确认结束后才报告 `Cancelled`；commit 确认成功后不再次检查取消，成功不会翻转。

- `CommitUnknown`：事务已清理，但 I/O 类错误使提交事实无法确认；不能伪称已回滚，也不自动重试。
- `CleanupFailed`：rollback 失败或事务未关闭；连接标记 retired，所有 clone、Block/history 视图拒绝后续 IO。调用方必须报告真实 cleanup failure，不能假释放 Other 为完成。
- 普通 SQL/type/range/schema 错误保持具体错误，不一律升级为 unknown。

history 同样区分 `CommitUnknown` 与 `CleanupFailed`。存储错误不覆盖已完成的工具结果，不重跑外部效果。

## 验证与插件边界

`cargo test -p memory_plugin` 当前 27 项通过，包含重启持久化、完整消息与 canonical IDs、TEMP 隔离/幂等、跨实例 lifecycle 冲突、schema/range/type/SQL rollback、self-source、跨 connection 多 GET 一致 snapshot、async handle 与 history 同 manager，以及 SQLite authorizer 真实拒绝 ROLLBACK 后的 CleanupFailed/retired 行为。测试不宣称覆盖真实磁盘故障的全部提交歧义，或代替 Driver/Other 的端到端生命周期验证。

可选 `MemoryPlugin::new()` 只安装 `MemoryPluginInstalled` 标记，无需 RuntimePlugin 或 schedule。本 crate 不导出 `AgentMemory`、`HistoryMessage`、实时 state API、旧错误或写入事件，不迁移或删除旧 `history_messages`、`setting`、`state` 表。
