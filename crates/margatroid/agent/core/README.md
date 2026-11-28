# agent_core

`agent_core` 提供 Agent 组件、生命周期请求、句柄与状态操作。
包依赖 mecs 的 `core_plugin` 和通用库，供生命周期编排、MCL 和观察适配使用。

## 核心对象

- `Agent` 保存 Workspace、父实体、创建任务、实例标识、项目目录、状态与 turn。
- `WorldAgentExt` 在 World 中执行 ready、turn 与关闭状态转换，并发布实际变化。
- `StartAgent<P>` 为具体持久化值保留泛型入口；`StopAgent` 按地址请求关闭。
- `AgentHandle` 将实体引用、ready 通知和异步关闭函数交给创建任务。
- `AgentUpdateContext` 管理来源安装/关闭通知与公共信息更新，`scoped` 产生实例作用域。

## 操作一个 Agent 的回合

下面的内存示例展示状态与 turn 的最小调用；完整运行实体由 agent_plugin 安装。
示例可放入同时依赖 `agent_core`、`core_plugin` 的 Rust 包。

```rust
use agent_core::{Agent, AgentState, WorldAgentExt};
use core_plugin::World;

fn main() -> Result<(), agent_core::AgentError> {
    let mut world = World::new();
    let workspace = world.spawn();
    let entity = world.spawn();
    world.insert_component(entity, Agent::new(
        workspace, workspace, "driver-task".into(), "/project".into(),
    ));
    world.agent_ready(entity)?;
    assert_eq!(*world.agent_state(entity)?, AgentState::Running);
    let turn = world.agent_start_turn(entity)?;
    assert_eq!(world.agent_start_turn(entity)?, turn);
    world.agent_finish_turn(entity, &turn)?;
    world.agent_begin_close(entity)?;
    assert_eq!(*world.agent_state(entity)?, AgentState::Closing);
    Ok(())
}
```

`agent_start_turn` 要求 Running，已有 turn 时返回同一标识。
`agent_abort_turn` 要求匹配当前 turn；`agent_finish_turn` 接受匹配值，
也接受中止后已经清空的回合。Closing 状态下的回合操作返回 `StateRejected`。

## 继续阅读

- [公开入口](src/lib.rs)：World 扩展和导出类型。
- [组件](src/components.rs)：Starting → Running → Closing 的转换与 ready 通知。
- [事件](src/events.rs)：创建请求、停止请求与 oneshot 回复。
- [回合测试](tests/turns.rs)：开始、结束、中止与旧实体处理。
- [生命周期契约测试](tests/lifecycle_contract.rs)：句柄与关闭能力。
- [Plugin 接入](../plugin/README.md) 与 [领域设计](../DESIGN.md)。

执行 `cargo test -p agent_core` 验证 core 的状态契约。
