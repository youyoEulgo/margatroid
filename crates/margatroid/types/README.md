# Margatroid Types

`margatroid_types` 保存多个 Margatroid Plugin 共享、没有业务执行逻辑的领域值和内部事件格式。
它可以依赖 `core_plugin` 的 `Entity` 与 `Event`，但不依赖任何 Margatroid 业务 Plugin，也不承载
CLI/daemon 网络 DTO。

资源层提供统一的 `ResourceId`。所有可寻址对象都视为资源，不论它是否由 ECS Entity
承载：

```rust
use margatroid_types::ResourceId;

let skill = ResourceId::parse("skill:local/code-review")?;
assert_eq!(skill.to_string(), "skill:local/code-review:latest");

let image = ResourceId::parse("image:local/coder:latest")?;
assert_eq!(image.resource_type(), "image");
```

资源ID固定使用 `type:scope/name:tag`。省略tag时规范化为 `latest`。Agent、Workspace、AgentImage、
Skill、Workflow和Tool都使用同一种身份格式。`ResourceId`是跨Plugin查找资源的统一地址；
`core_plugin::Entity`只作为当前World内的运行时句柄，不进入协议、日志或Memory。

Compose使用 `WorkspaceDefinition` 和 `WorkspaceAgentDefinition` 把配置文件编译成进程内静态业务
输入。它们不包含运行时Entity，也不负责加载镜像、打开Memory或创建AgentInstance。

消息层定义 `ToolCall`、`ToolDefinition`、`Message`、`AgentMessage`、`AgentContextMessagesUpdated`、
`AgentHistoryMessageWriteRequested` 和 `AgentFailure`。AgentPlugin直接根据`Message`变体与
`tool_calls`列表决定后续分支。这些是进程内部ECS事件，不是CLI/daemon协议。
`AgentMessage.agent` 始终使用后端内部 `Entity`。WorkspacePlugin 在产生用户消息事件前按完整
Agent资源ID解析，协议 DTO 使用稳定 Agent ID，不暴露 ECS 内部句柄。

`Message::User.tool_calls`直接保存前端预选调用。列表非空时AgentPlugin先执行工具，
所有Tool响应进入当前轮上下文后再推理；列表为空时直接推理。

AgentPlugin使用`BTreeSet<ResourceId>`保存Agent可见性。`ResourceId.resource_type`负责选择工具
定义Plugin，普通工具、Skill和Workflow不再建立不同通道。每次请求由AgentPlugin遍历该集合，并把单个
`ResourceId`交给对应Provider构造工具。Provider不得忽略tag。

历史写入统一使用`AgentHistoryMessageWriteRequested`。Skill正文不进入历史事件，只写入
`skill: <scope/name> loaded`标记。

无法表示成消息的轮次失败使用 `AgentFailure`。`kind=Agent` 表示 AgentPlugin 在处理消息或准备可见工具
时失败，`kind=Inference` 表示 InferencePlugin 在准备或执行模型请求时失败。
