# MemoryPlugin

`MemoryPlugin`为每个Agent实例绑定一个SQLite数据库。两张表有不同且不能互换的用途：

- `history_messages` 是客户端可展示对话的权威来源，追加 User、Assistant 和 Tool。
  Assistant同时保存本次输入、输出和缓存命中Token；Skill Tool只保存资源ID，不保存Skill正文。
- `realtime_context` 由 Base Lua Driver 通过显式 `realtime_source (req)` effect 声明来源；每次 request block 快照变化时整体替换，按顺序保存 `message` 及可选的 Assistant 用量。

```rust
use app_runtime_plugin::RuntimePlugin;
use core_plugin::App;
use memory_plugin::{AgentMemory, MemoryPlugin, WorldMemoryExt};

let mut app = App::new();
app.add_plugin(RuntimePlugin::default())
    .add_plugin(MemoryPlugin::default());

let agent = app.world_mut().spawn();
let (memory, context) = AgentMemory::open("/project/.margatroid/memory/agent.sql")?;
app.world_mut()
    .bind_agent_memory(agent, memory, &context)?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

Base Lua通过显式`history_append` effect请求追加历史，不直接调用SQLite；AgentPlugin只补齐工具
schema等领域元数据。Base Lua通过`realtime_source (req)`声明权威请求Block，MCL在该快照变化后发送
`AgentRealtimeContextWriteRequested`，MemoryPlugin在事务中整体替换有序实时上下文。
打开数据库时，MemoryPlugin还会汇总全部历史Assistant行的Token列，供Agent恢复累计状态。

客户端不得从 `realtime_context` 恢复对话，也不得把实时 Agent 事件自行拼入展示历史。daemon 读取
`history_messages` 后通过协议发送完整历史快照，客户端直接以该快照替换当前展示内容。
