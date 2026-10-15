# Margatroid Types

`margatroid_types` 保存多个 Margatroid Plugin 共享、没有业务执行逻辑的领域值和内部事件格式。
它可以依赖 `core_plugin` 的 `Entity` 与 `Event`，但不依赖任何 Margatroid 业务 Plugin，也不承载
CLI/daemon 网络 DTO。

资源层提供统一的 `ResourceId`。所有可寻址对象都视为资源，不论它是否由 ECS Entity
承载：

```rust
use margatroid_types::ResourceId;

let tool = ResourceId::parse("tool:local/code-review")?;
assert_eq!(tool.to_string(), "tool:local/code-review:latest");

let image = ResourceId::parse("image:local/coder:latest")?;
assert_eq!(image.resource_type(), "image");
```

资源ID固定使用 `type:scope/name:tag`。省略tag时规范化为 `latest`。Agent、Workspace、AgentImage、
Prompt、Sandbox和Tool都使用同一种身份格式。`ResourceId`是跨Plugin查找资源的统一地址；
`core_plugin::Entity`只作为当前World内的运行时句柄，不进入协议、日志或Memory。

Compose使用 `WorkspaceDefinition` 和 `WorkspaceAgentDefinition` 把配置文件编译成进程内静态业务
输入。它们不包含运行时Entity，也不负责加载镜像、打开Memory或创建AgentInstance。

## 共享消息与执行边界

`AgentMessage` 是纯消息值，不是带 Entity 的路由事件；包含 Inject、System、User、Assistant、ToolCall 结果及 Error。`ToolCall` 输入只存在于 Assistant.tool_calls。资源可见性由 Agent 的显式导入绑定与 MESSAGE/TOOL/SANDBOX typed arrays 决定，不恢复旧 AgentFailure、邮箱或可见性集合。

## 用户轮次中止目标（待实现）

本节及下文标注的中止行为是已批准的目标契约，不表示当前代码已实现；既有实现说明仅适用于未被该目标替换的部分。无 `get_turn`。状态为 `creating/idle/working/stopping`；中止立即清空 turn，清理期间保持 Working，正常 finish 收口后才 Idle；状态同步同时发布 turn 与 status。用户轮次中止不等于根 Agent shutdown、owner abort、timeout 或 receiver closed。

Assistant 新增 `stop_reason: StopReason`，覆盖所有推理原因，包含新增 `StopReason::Cancelled`（序列化 `cancelled`）。兼容保留 `InferenceResponse.stop_reason`，同一权威映射一次构造两处，必须相同。用户轮次中止且真实清理成功返回 Ok assistant：`content="Turn aborted."`、`reasoning=None`、`tool_calls=[]`、`usage=None`、`stop_reason=Cancelled`。

取消工具且真实清理成功返回 Ok ToolCall 结果：`failed=true`、`content="Tool call aborted."`，保留调用 ID、名称及已知 resource_id；未启动项同用该正文。批次每个调用均有一个结果，保留已完成结果，不虚构部分成功，不撤销已发生副作用。`CleanupFailed` 仍为 Err，保留活动，不宣称空闲。

AbortSystem 必须先于 MCL 实际执行与 inference/tool admission。共享中止事件允许多个消费者独立观察，不靠 take reply 独占通知；实际执行时重新核对请求 turn，turn=None 或已中止旧轮次返回正常取消结果，不启动执行。

历史仅由显式 MCL `history_append` 保存。Assistant stop_reason 存在历史 payload 元数据，不混入正文；cancelled assistant 可入历史但不可进入 infer 上下文（包括 Inject 嵌套）。领域请求、结果与错误保持各自 oneshot/共享事件关联，不恢复 AgentFailure 路由。
