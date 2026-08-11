# MemoryPlugin

`MemoryPlugin`为每个Agent实例绑定一个SQLite数据库。两张表有不同且不能互换的用途：

- `history_messages` 是客户端可展示对话的权威来源，追加 User、Assistant 和 Tool。
  Skill Tool只保存`skill: <scope/name> loaded`，不保存Skill正文。
- `realtime_messages` 使用`conversation`和`tool`两个分区，分别与 `AgentContext.messages` 和
  `AgentContext.tool_context` 同步。该表不用于客户端展示。

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

AgentPlugin发送`AgentHistoryMessageWriteRequested`追加历史，不直接调用SQLite。Agent上下文
修改后发送`AgentContextMessagesUpdated`，MemoryPlugin在事务中整体替换两个实时分区。

客户端不得从 `realtime_messages` 恢复对话，也不得把实时 Agent 事件自行拼入展示历史。daemon 读取
`history_messages` 后通过协议发送完整历史快照，客户端直接以该快照替换当前展示内容。
