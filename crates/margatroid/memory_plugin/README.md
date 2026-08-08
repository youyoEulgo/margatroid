# MemoryPlugin

`MemoryPlugin`为每个Agent实例绑定一个SQLite数据库。两张表有不同且不能互换的用途：

- `history_messages` 是客户端可展示对话的权威来源，只追加 User 和 Assistant 消息。Tool 响应、
  System 消息以及 Skill、Workflow 等资源正文不进入该表；实际使用的资源只以引用形式写入
  `resources` 列。
- `realtime_messages` 是恢复模型动态上下文的权威来源，始终与 `AgentContext.messages` 完全同步，
  可以包含 User、Assistant 和 Tool 消息，但不包含 System 消息。该表不用于客户端展示。

```rust
use app_runtime_plugin::RuntimePlugin;
use core_plugin::App;
use memory_plugin::{AgentMemory, MemoryPlugin, WorldMemoryExt};

let mut app = App::new();
app.add_plugin(RuntimePlugin::default())
    .add_plugin(MemoryPlugin::default());

let agent = app.world_mut().spawn();
let (memory, messages) = AgentMemory::open("/project/.margatroid/memory/agent.sql")?;
app.world_mut()
    .bind_agent_memory(agent, memory, &messages)?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

AgentPlugin通过`WorldMemoryExt::append_history_message`同步追加User和Assistant历史。Tool消息不调用该
入口；Agent上下文修改后发送`AgentContextMessagesUpdated`，MemoryPlugin在事务中整体替换实时表。

Skill、Workflow等资源正文不进入数据库。资源Plugin只发送`AgentResourcesUsed`，MemoryPlugin将
`ResourceRef`合并到对应User历史行的`resources`列。

客户端不得从 `realtime_messages` 恢复对话，也不得把实时 Agent 事件自行拼入展示历史。daemon 读取
`history_messages` 后通过协议发送完整历史快照，客户端直接以该快照替换当前展示内容。
