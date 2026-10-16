# WorkspacePlugin

`WorkspacePlugin` 接收 Compose 生成的 `WorkspaceDefinition`，负责 Workspace 及其 Agent 的创建、消息路由、
MCL 命令转发和停止收敛。每个 Workspace 与 Agent 都获得全局唯一的资源地址，例如 `workspace:demo` 与
`agent:demo/coder:latest`；该地址用于跨 Plugin 和 API 引用，ECS Entity 只在后端内部使用。

创建过程完全由事件驱动，不阻塞调用方：

```text
StartWorkspace             创建 Workspace Entity，登记 ResourceId 与 Workspace 组件
                           为每个成员的 AgentDriver 发送 LoadAgentDriver
LoadAgentDriverResult       经 collect_agent_driver_system 读取驱动、打开 Memory
                           构造 AgentCreateRequest 并登记 pending_agents
AgentCreateRequest 回执    经 collect_agent_driver_system 消费，成功成员写入 Workspace.agents
AgentInitializationCompleted
                           经 collect_agent_initialization_system 标记 Ready
```

成员状态由 `WorkspaceAgentState` 表达，依次经过 `Creating`，再到 `Ready` 或 `Failed`。单个成员失败会记录错误，
不影响同一 Workspace 中的其他成员。

停止不是立即销毁，而是先登记意图、再逐帧收敛。`stop_workspace_entity` 把 Workspace 记入
`WorkspaceRegistry.stopping` 并向成员发送 `AgentControl::Stop`；`collect_workspace_stop_system` 每帧检查成员是否
已无在途工作（Lua VM、工具调用、推理均为空且生命周期落定），确认后才释放其持有的资源并回收实体。

停止请求不立即回复，而是压入 `WorkspaceStopState.replies`，等整个 Workspace 排空后才逐个转成结果事件。
因此调用方遇到的超时等价于"该 Workspace 没有排空"。

停止收敛的完整判据、以及判据长期不满足时的后果与排查顺序，见 `DESIGN.md` 的 `# handler` 板块。
