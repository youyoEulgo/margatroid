# MemoryPlugin

`MemoryPlugin`为每个Agent实例绑定一个SQLite数据库。`history_messages`只追加User和Assistant消息；
`realtime_messages`始终保存当前动态上下文，包含User、Assistant和Tool消息但不包含System消息。

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
