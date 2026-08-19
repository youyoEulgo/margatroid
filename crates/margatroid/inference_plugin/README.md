# InferencePlugin

## 介绍

`InferencePlugin` 是 Margatroid 统一 `Message` 与具体模型 API 之间的边界。Agent 核心只维护
`messages`，不识别 OpenAI、OpenRouter 或其他 Provider 格式。

这里的 `inference` 指一次模型推理调用：输入 `messages`、工具定义和推理参数，输出一个完整
Assistant 响应。它不是 Agent 的完整思考过程，不包含工具执行或 tool-call loop。

Plugin 根据 AgentInstance 绑定的逻辑 `ModelId` 按项目级、全局顺序查询模型路由，将统一消息
组装为 Provider Request，发送流式 HTTP 请求，将文本片段直接写入模型路由指定的 WebSocket，最后
返回完整统一响应。

Provider 请求失败时，错误会保留安全 endpoint、传输错误类别和最深层系统原因。非成功 HTTP 响应会
优先提取常见 JSON 错误字段，并回退到有界、单行化的响应正文。错误不会包含 Authorization header、
API key、请求正文、URL 凭据或查询参数，最终文本限制为 512 字节。

AgentImageLoaderPlugin 只提供中立模型配置；InferencePlugin 负责把它转换为实例推理快照，
并验证推理参数和模型路由。资源加载层不依赖推理业务层。

这里的关键边界是：

- Agent 传递统一 `Message`，InferencePlugin 解释模型协议。
- `ModelId` 是逻辑路由键，不代表实际模型或供应商。
- `provider` 是可选元数据，`api_type` 才决定请求和响应协议。
- ProviderAdapter 组装请求，ProviderResponseAccumulator 累积并解析响应。
- 流式思考与正文通过固定的 WebSocket 发送器集合分别直接转发，不为每个分片创建 ECS Event。
- InferencePlugin 不修改 Agent 的 `messages`，不执行工具，不实现 tool-call loop。

上下文压缩使用独立的`ContextCompactionInferenceRequest`和`ContextCompactionInferenceResponse`。
它复用相同模型路由、Provider Adapter、HTTP与取消机制，但不携带工具定义、不解析流式前端目标，也不生成
普通`AgentMessage`。只有正常结束、没有工具调用且正文非空的响应才作为摘要返回AgentPlugin。

普通推理会通过`stream_options.include_usage`请求Provider在流末尾返回usage，并把输入、输出和缓存命中
Token随`AgentMessage`交给AgentPlugin；Provider完全不返回usage时保持为空。

## 模型路由表

默认配置文件为 `~/.margatroid/models.toml`：

```toml
[[models]]
id = "deepseek-v4-flash"
model = "deepseek/deepseek-v4-flash-latest"
provider = "openrouter"
base_url = "https://openrouter.ai/api/v1"
api_key = "sk-123"
api_type = "deepseek"
thinking = "enabled"
reasoning_effort = "high"
context_window = "200k"
```

`context_window`表示模型总上下文窗口，使用AI领域常见的英语数量级缩写且不区分大小写：
`k=thousand`、`m=million`、`b=billion`、`t=trillion`。例如`200k`为200000 Token、
`1m`为1000000 Token、`1b`为1000000000 Token。省略时默认`1m`。该值与只限制
生成长度的`max_output_tokens`不同，并会标准化后提供给Agent Base Driver。

`api_type = "deepseek"` 启用DeepSeek协议Adapter。`thinking = "enabled"`时请求携带
`thinking = { type = "enabled" }`，`reasoning_effort`可为`high`或`max`；省略thinking或设为
`disabled`时不发送思考参数。即使`base_url`指向OpenAI兼容反代，只要需要DeepSeek的
`reasoning_content`语义，仍应选择`deepseek`。
响应解析同时接受DeepSeek原生`reasoning_content`和OpenRouter兼容`reasoning`字段；请求历史回传
始终使用DeepSeek语义的`reasoning_content`。

AgentImage 只引用 `id = "deepseek-v4-flash"`。ID 通常直接写具体模型名，方便开发者辨认；
`model` 则是构建 Provider Request 时直接使用的模型值。`base_url`、`api_key` 和 `api_type`
也只属于路由表，`provider` 可以省略且不参与路由查找。WebSocket 发送目标不属于模型路由，统一
来自主目录 `config.toml`。

项目可以在 `<project>/.margatroid/models.toml` 使用相同格式定义项目级路由。查找顺序是：

```text
WorkspaceModelRoutes[ModelId]
→ GlobalModelRoutes[ModelId]
→ ModelRouteNotFound
```

主目录配置是全局默认，项目级同名 ID 会覆盖它。项目级路由挂在 Workspace Entity 上，
因此不同 Workspace 可以为同一 `ModelId` 使用不同实际模型。

ID 是面向 AgentImage 和开发者的稳定名称，`model` 是面向 Provider 的请求值，因此二者不要求
字面相同。例如 `model` 可以包含供应商作用域或版本后缀，也可以在迁移、测试时指向其他兼容模型。

修改某条项目级路由会影响该 Workspace 中引用它的 Agent；修改全局路由会影响所有没有
项目级同名覆盖的 Agent。如果同一 Workspace 内也需要不同路由，应新增 ID，而不是在
AgentInstance 内保存私有 Provider 配置。

`api_key` 只在配置加载、ProviderAdapter 和实际 HTTP 请求中使用，不得进入 Component、
Event、Error 或日志。

## 安装

`InferencePlugin` 依赖 `RuntimePlugin`、`AsyncRuntimePlugin` 和 `AgentImageLoaderPlugin`：

```rust
use app_runtime_plugin::RuntimePlugin;
use agent_image_loader_plugin::AgentImageLoaderPlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use core_plugin::App;
use inference_plugin::InferencePlugin;

let mut app = App::new();
app.add_plugin(RuntimePlugin::default())
    .add_plugin(AsyncRuntimePlugin)
    .add_plugin(AgentImageLoaderPlugin::open(
        "/home/user/.margatroid/agent-images",
    )?)
    .add_plugin(InferencePlugin::default());
```

默认在 `RuntimePlugin::PRE_UPDATE` 处理推理命令、重载命令和异步结果。可以在安装时
替换配置路径或 Schedule：

```rust
use std::path::PathBuf;

let inference = InferencePlugin::default()
    .with_config_path(PathBuf::from("/etc/margatroid/models.toml"))
    .with_schedule(RuntimePlugin::UPDATE);

app.add_plugin(inference);
```

Plugin 安装时会加载并验证完整路由表。缺少文件、重复 ID、非法 URL、未注册
`api_type` 或缺少必要字段都会导致配置失败。

Plugin 安装只加载主目录全局默认路由。WorkspacePlugin 在 `workspace up/reload` 时调用
`load_workspace_model_routes(workspace, project_root)`，将可选项目级路由挂到 Workspace Entity。

## AgentImage 推理配置

AgentImage 必须由 AgentImageLoaderPlugin 创建。Loader 解析模型 ID 和参数原值后，在
AgentImage Entity 上挂载中立的 `AgentImageModelConfig`；它不调用 InferencePlugin，也不验证
温度范围、停止序列业务规则或模型路由。

```text
AgentImage文件
→ AgentImageLoaderPlugin创建AgentImage Entity
→ 挂载AgentImageModelConfig { model: "deepseek-v4-flash", parameters }
→ WorkspacePlugin调用world.build_agent_inference_snapshot(...)
→ InferencePlugin验证参数和当前模型路由
→ WorkspacePlugin把AgentInferenceSnapshot挂到新AgentInstance
```

Workspace 启动时先加载项目级路由，再调用：

```rust
use agent_image_loader_plugin::AgentImageModelConfig;
use inference_plugin::WorldInferenceExt;

let config = world
    .get_component::<AgentImageModelConfig>(image)
    .expect("loaded image must contain model config");

let snapshot = world.build_agent_inference_snapshot(workspace, image, config)?;
world.insert_component(agent, snapshot);
```

该方法把模型 ID 文本转换为 `ModelId`，把原始参数转换为 `InferenceParameters`，验证业务范围，
并按项目级、全局顺序确认路由存在。它只返回组件，不创建 AgentInstance，也不修改 World。

AgentImage 文件后续变化不会自动修改已启动实例；重新读取 AgentImage 和项目级模型路由都属于
`workspace reload`。快照记录 Workspace Entity，供推理时优先查询项目级路由。

## 发起推理

Agent 核心发送当前完整 `messages` 快照。下面的 `agent` 由 Workspace 启动逻辑创建，
并已挂载 `AgentInferenceSnapshot`：

```rust
use app_runtime_plugin::WorldEventExt;
use inference_plugin::InferenceRequest;
use margatroid_types::{Message, ToolDefinition};

app.world().send_event(InferenceRequest {
    id: "request-1".into(),
    agent,
    agent_id: ResourceId::parse("agent:demo/coder:latest")?,
    messages: vec![
        Message::System {
            content: "You are a coding agent.".into(),
        },
        Message::User {
            content: "Review this patch.".into(),
        },
    ],
    tools: Vec::<ToolDefinition>::new(),
});
```

`id` 由调用方生成，用于区分同一 Agent 的并发请求；`agent` 是发起推理的 AgentInstance
Entity。最终响应使用 `(agent, id)` 定位原 Agent 和原请求。

`tools`由AgentPlugin遍历当前`AgentDynamicVisibility.resources`，逐个调用ToolPlugin按资源类型返回定义；
每个定义名都是完整`ResourceId`。InferencePlugin在主线程读取Agent推理快照，并在请求边界将资源ID转换为模型API工具名，根据`ModelId`组装请求，然后
把网络请求交给 AsyncRuntime。不会把 `World` 或 Component 引用带入异步线程。

## 流式输出与最终结果

后端对一次模型调用的有效处理都依赖完整响应：工具参数需要拼完才能解析，tool-call loop
需要完整 Assistant Message，结束原因和 token usage 也只有流结束后才能确定。

流式思考和正文的用途只是让前端实时显示。`InferenceRequest` 不携带发送器；`prepare_inference_system`
读取全局配置的 `streaming_member_messages` 目标，通过 `WebSocketConnections` 解析并固定本轮的发送器集合，然后写入
`PreparedInference`。异步推理解析出的正文构造`agent.message.delta`，DeepSeek思考内容构造
`agent.message.reasoning_delta`，两者都通过`WebSocketMessageSender::send`直接发送。

思考和正文片段都不进入 ECS 事件队列；工具调用 ID、名称和参数片段也不转发，只在后端累积器中组装。
前端已断开时，InferencePlugin 停止向对应连接转发，但不中断后端推理和最终响应累积。本轮开始后
新增且符合相同 target 的连接从下一轮开始接收，以保持增量与最终消息的接收集合一致。

流结束后，成功响应由 InferencePlugin 直接转换成共享 `AgentMessage::Assistant`。InferencePlugin
不为响应附加意图，也不判断本轮是否结束；AgentPlugin 收到消息后根据 `tool_calls` 是否为空决定
结束当前轮次或派发下一批工具调用：

```rust
use core_plugin::World;
use margatroid_types::{AgentMessage, Message};

fn handle_inference_messages(world: &mut World) {
    for event in world.event_reader::<AgentMessage>() {
        if let Message::Assistant { reasoning, content, tool_calls } = &event.message {
            tracing::info!(
                agent = ?event.agent,
                request_id = %event.id,
                ?reasoning,
                ?content,
                tool_call_count = tool_calls.len(),
                "agent inference completed"
            );
        }
    }
}

app.add_system(RuntimePlugin::UPDATE, handle_inference_messages);
```

无法表示成对话消息的推理失败通过 `AgentFailure { id, agent, kind, message }` 发布。AgentPlugin
直接消费这两种共享事件：它将合法 Assistant 消息写入对应实例上下文，并根据消息结构继续工具
调用或结束当前轮次；可配对的失败用于结束当前推理状态。InferencePlugin 不参与这些分支判断。

## 重载模型路由

InferencePlugin 不监听文件变化。修改主目录全局 `models.toml` 后显式请求重载：

```rust
use inference_plugin::WorldInferenceExt;

app.world().reload_model_routes("reload-models-1");
```

`reload_model_routes` 发送事件并唤醒 Runtime。内部 System 同步读取、验证并替换全局
路由表；这是低频管理操作，允许阻塞当前帧。处理结果从 `ReloadModelRoutesResult` 读取：

```rust
use inference_plugin::ReloadModelRoutesResult;

fn handle_model_route_reload(world: &mut World) {
    for event in world.event_reader::<ReloadModelRoutesResult>() {
        match &event.result {
            Ok(reloaded) => tracing::info!(
                route_count = reloaded.route_count,
                "model routes reloaded"
            ),
            Err(error) => tracing::error!(%error, "model route reload failed"),
        }
    }
}

app.add_system(RuntimePlugin::UPDATE, handle_model_route_reload);
```

项目级 `models.toml` 不走这个全局重载入口，它随 `workspace up/reload` 重新加载。全局重载
只影响未被项目级同名 ID 覆盖的 Agent。

## 扩展 API 协议

默认提供`openai`和`deepseek`协议工厂。新协议通过`InferencePlugin::with_api_type`注册：

```rust
let inference = InferencePlugin::default()
    .with_api_type("anthropic", AnthropicAdapterFactory::new());
```

一种新 API 协议需要实现三个边界：

- `ProviderAdapterFactory`：读取 `provider`、`base_url` 和 `api_key`，创建已配置 Adapter。
- `ProviderAdapter`：将统一 `ProviderInput` 组装为 HTTP Request，并为每个响应创建累积器。
- `ProviderResponseAccumulator`：接受任意网络分片边界，返回有序的`ProviderStreamDelta`，在内部累积
  思考、正文和工具调用并最终构造`ProviderInferenceResponse`；`finish`也会返回缓冲区尾行解析出的分片。

Adapter 只解释 Provider 协议，不读取 `World`、Agent Component 或会话状态。

## 数据流

```text
Agent messages + AgentPlugin收集的ToolDefinition
→ InferenceRequest { id, agent, agent_id, messages, tools }
→ AgentInferenceSnapshot { workspace, model }
→ WorkspaceModelRoutes[ModelId]
→ 未命中时查询GlobalModelRoutes[ModelId]
→ ProviderAdapter::build_request
→ AsyncRuntime发送流式HTTP
→ ProviderResponseAccumulator::push
├→ Reasoning/Content分片 -> WebSocketMessageSender -> 前端
└→ 后端内部累积完整响应
→ ProviderResponseAccumulator::finish
├→ 尾行分片 -> WebSocketMessageSender -> 前端
├→ 成功：AgentMessage { id, agent, Message::Assistant }
└→ 失败：AgentFailure { id, agent, kind: Inference, message }
→ 成功消息由AgentPlugin记入上下文并根据tool_calls继续工具调用或结束；失败契约暂不定义
```

## 职责边界

InferencePlugin 负责：

- 加载、验证和重载主目录全局默认路由。
- 提供将项目级路由覆盖加载到 Workspace Entity 的公开入口。
- 把 AgentImageLoaderPlugin 的中立模型配置转换为经过验证的 AgentInferenceSnapshot。
- 将统一 messages、工具定义和推理参数组装为 Provider Request。
- 发送流式 HTTP 请求并解析增量与最终响应。
- 将思考和正文片段写入固定 WebSocket 发送器集合，并按请求 ID 和 Agent Entity 发布 `AgentMessage` 或
  `AgentFailure`。
- 根据完整 Assistant 消息是否包含工具调用，直接赋予消息意图。

InferencePlugin 不负责：

- 创建 Workspace、AgentImage 或 AgentInstance。
- 持有或修改 Agent 的 `messages`。
- 执行工具或实现 tool-call loop。
- 读取 Skill、Workflow 或 Memory。
- 决定重试、上下文裁剪或对话结束策略。

完整类型、函数和执行逻辑见 [DESIGN.md](DESIGN.md)。
