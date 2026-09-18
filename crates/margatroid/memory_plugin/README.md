# MemoryPlugin

`MemoryPlugin`为每个Agent实例绑定一个SQLite数据库。两张表有不同且不能互换的用途：

- `history_messages` 是客户端可展示对话的权威来源，追加 User、Assistant 和 Tool。
  Assistant同时保存本次输入、输出和缓存命中Token；Skill Tool只保存资源ID，不保存Skill正文。
- `setting` 保存 MCL driver 通过 `BIND <block> TO STATE <name>` 声明的动态 state，值是完整 block 的 JSON。实时上下文也是普通 state，由 driver 声明 `realtime_state` block 后通过 `LOAD STATE realtime` 恢复、通过 `BIND realtime_state TO STATE realtime` 持久化。

```rust
use agent_plugin::AgentMemoryHandle;
use app_runtime_plugin::RuntimePlugin;
use core_plugin::App;
use memory_plugin::{AgentMemory, MemoryPlugin};
use std::sync::Arc;

let mut app = App::new();
app.add_plugin(RuntimePlugin::default())
    .add_plugin(MemoryPlugin::default());

let memory = AgentMemory::open("/project/.margatroid/memory/agent.sql")?;
let handle = AgentMemoryHandle::new(Arc::new(memory));
# Ok::<(), Box<dyn std::error::Error>>(())
```

Base Lua通过显式`history_append` effect请求追加历史，不直接调用SQLite；AgentPlugin只补齐工具
schema等领域元数据。state 的读取和写入由 MCL 的 `LOAD STATE`、`BIND ... TO STATE` 完成，driver
负责决定何时将消息 block 同步到 `realtime_state`，MemoryPlugin只提供通用 state blob 存储。

客户端不得把 state 当作展示历史，也不得把实时 Agent 事件自行拼入展示历史。daemon 读取
`history_messages` 后通过协议发送完整历史快照，客户端直接以该快照替换当前展示内容。
