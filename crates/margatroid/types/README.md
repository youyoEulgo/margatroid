# Margatroid Types

`margatroid_types` 保存多个 Margatroid Plugin 共享、没有业务执行逻辑的领域值和内部事件格式。
它可以依赖 `core_plugin` 的 `Entity` 与 `Event`，但不依赖任何 Margatroid 业务 Plugin，也不承载
CLI/daemon 网络 DTO。

资源层提供 `ResourceName` 和 `AgentImageReference`：

```rust
use margatroid_types::ResourceName;

let name = ResourceName::new("local/code-review")?;
assert_eq!(name.scope(), "local");
assert_eq!(name.name(), "code-review");

let image = margatroid_types::AgentImageReference::new("local/coder")?;
assert_eq!(image.to_string(), "local/coder:latest");
```

资源名称固定使用 `scope/name`。Skill、Workflow 和 AgentImage Loader 共享这一类型，从而不会各自
维护一套名称解析规则。AgentImage 引用在名称后增加可选 `:tag`，省略时规范化为 `latest`。

Compose使用 `WorkspaceDefinition` 和 `WorkspaceAgentDefinition` 把配置文件编译成进程内静态业务
输入。它们不包含运行时Entity，也不负责加载镜像、打开Memory或创建AgentInstance。

消息层定义 `ToolCall`、`ToolDefinition`、`Message`、`AgentMessage`、`AgentContextMessagesUpdated` 和 `AgentFailure`。产生消息的
Plugin直接赋予 `MessageIntent`，AgentPlugin记入消息并执行意图，不再把各Plugin结果事件二次转换为
消息。`AgentMessage` 和 `AgentContextMessagesUpdated` 是进程内部ECS事件，不是CLI/daemon协议。

用户消息的两种意图只区分前端是否指定了实际工具调用。指定Skill、Workflow或其他工具时，
AgentPlugin先进入ToolCall流程，等Tool响应写入上下文后再推理；没有指定时直接推理。两种路径最终
发送的推理请求都按当前动态可见性构造工具定义，意图不用于开关模型可用工具。

AgentPlugin使用`BTreeSet<ResourceRef>`保存Agent可见性。`ResourceRef.provider`只负责路由工具定义
Plugin，普通工具、Skill和Workflow不再建立不同通道。每次请求由AgentPlugin遍历该集合，并把单个
`ResourceRef`交给ToolPlugin构造工具。

资源来源可以用 `MessageResource` 与 `AgentResourcesUsed` 报告一次Agent轮次实际使用的资源。
事件只携带统一`ResourceRef`，不携带Skill、Workflow或其他资源正文。

无法表示成消息的轮次失败使用 `AgentFailure`。`kind=Agent` 表示 AgentPlugin 在处理消息或准备可见工具
时失败，`kind=Inference` 表示 InferencePlugin 在准备或执行模型请求时失败。
