# InferencePlugin

> 状态：格式/vendor分层重构已实现。共享拥有所有权的领域请求、三种格式转换与严格SSE累积器、必填vendor和显式七组合路由已落地；执行、资源及真实cleanup边界保持不变。用户图像通过共享校验后转换为三种格式的原生图像块。

## 已实现的请求分层

- **共享纯领域请求**：专用 `types::inference_request_types` 中仅有单一 `DomainRequest { messages: Vec<AgentMessage>, tools: Vec<ToolDefinition>, config: DomainRequestConfig }`。`DomainRequestConfig` 包含上游 `model: String`、`temperature`、`max_output_tokens`、`top_p`、`stop` 公共语义参数；默认空model在执行前拒绝。请求拥有所有权，不含HTTP、URL、密钥、provider、执行状态、Entity或channel。它与插件 `InferenceConfig` Resource不同。资源归属检查及CallableResource快照准备仍先完成，再构造纯ToolDefinition。
- **执行事件不混合领域值**：`InferenceRequest` 仍承担请求关联、Entity、turn、cancel和oneshot等执行职责；不是把其改名为DomainRequest。现有工具快照、客户端快照、独立supervisor、真实cleanup和显式历史边界不变。
- **格式与厂商独立**：插件 `inference_plugin::types` 定义 `ApiFormat { ChatCompletions, Messages, Responses }`，稳定配置值分别为 `chat_completions/messages/responses`。provider从可选标签变为必填vendor执行选择器；model是独立上游模型名称，不推断格式或厂商。
- **具体格式再执行**：插件内部 formats 模块拥有 `ChatCompletionsRequest`、`MessagesRequest`、`ResponsesRequest` 及包含它们的 `ApiRequest`。`ApiFormat::build_request` 通过 `TryFrom<DomainRequest>` / `.try_into()?` 转换，vendors 模块的 `VendorAdapter` 接收具体格式的 `ApiRequest` 再处理厂商选项、鉴权、端点和HTTP。公开重新导出 `ApiFormat`、`Vendor`；不存在旧协议工厂注册表、`with_api_type`、`ApiAdapter` 或泛型vendor handler类型。
- **可失败而非静默损失**：领域请求保留完整消息及资源关联；格式转换检查工具调用/结果配对及格式可表达性，厂商层补充已验证的选项约束。内部resource ID不作为API字段发送，本地结果/历史仍保留。Responses不支持stop序列，Messages缺少max_output_tokens时拒绝；跨格式replay或缺少必要reasoning元数据也在HTTP前失败。不宣称已实现独立vendor/model能力快照系统。
- **响应逆向归一**：HTTP/SSE经vendor特有兼容及对应格式解码/累积，统一成 `InferenceResponse`。各格式使用自己的结束条件；不能都套用Chat Completions的choice/`[DONE]`。保留tool IDs、reasoning、usage及真实stop_reason；正常响应和用户轮次abort的统一构造helper保证内外stop_reason一致，且不改变既有真实cleanup条件。
- **普通无状态Responses**：首期每次提交完整本地上下文，不依赖远端conversation或previous_response_id，不用远端状态替代本地历史。

当前显式支持矩阵（七组合，2 + 1 + 3 + 1；不保证每个远端模型都支持全部语义）：

| provider | ApiFormat稳定配置值 |
| --- | --- |
| openai | chat_completions、responses |
| anthropic | messages |
| deepseek | chat_completions、messages、responses |
| openrouter | chat_completions |

未知provider、格式或未列出组合在加载/重载时拒绝；无URL猜测、无隐式openai兜底。具体vendor/model能力还可进一步限制输入。配置唯一来源仍为现有 `models.toml`，不是新增并行 `config.toml`；旧 `api_type = "openai"` / `"deepseek"` 已拒绝，必须改为实际格式值并明确provider。全部路由与catalog编译成功才原子发布；重载失败同时保留旧配置和旧catalog，在途请求保留原快照。

用户图像：`User { content, images: Vec<UserImage> }` 使用原始标准base64，空images默认并省略。handler预检及格式normalize均调用共享 `validate_user_images`：仅png/jpeg/gif/webp、每消息最多8张、每图5MiB、每消息解码总量20MiB，校验magic与MIME一致。仅User允许图像，System/Assistant/Tool不支持。顺序统一为非空文本在前、图像按输入顺序在后；纯图像不生成空文本块。Chat使用image_url data URL，Messages使用base64 image source，Responses使用input_image data URL；无图像时保持旧请求体。七组合的API形状支持不保证上游模型视觉能力，尤其不声明DeepSeek模型支持视觉。

显式message历史策略已实现：推理和工具调用只返回结果，所有message历史仅由显式MCL history_append控制。MCL指令审计仍自动记录，审计payload中的消息不作为独立message记录。示例Driver对需要保留的结果另发显式追加指令。


## 用户轮次中止（已实现）

受理核对 Working 及非空匹配 turn，活动登记关联 turn 并监督独立 turn_cancel 令牌。无 `get_turn`。状态为 `creating/idle/working/stopping`；中止立即清空 turn，清理期间保持 Working，正常 finish 收口后才 Idle；状态同步同时发布 turn 与 status。用户轮次中止不等于根 Agent shutdown、owner abort、timeout 或 receiver closed。

Assistant 新增 `stop_reason: Option<StopReason>`（旧文件缺字段默认为 None，prompt 非模型消息为 None，实际推理输出始终 Some），包括 `StopReason::Cancelled`（`cancelled`）；兼容保留外层 `InferenceResponse.stop_reason`，两处从单一权威映射构造且始终相同。仅用户轮次中止在真实 cleanup 成功后返回 Ok：先 abort 并 await 本地 HTTP/SSE worker，随后返回已接受的正文与 reasoning 部分快照，`tool_calls=[]`、`replay=None`、`usage=None`、`stop_reason=Cancelled`；没有任何正文或 reasoning 时，`content=None`、`reasoning=None`，保持空 assistant，不填充提示正文。快照在每批 ProviderStreamDelta 被接受后、任何 websocket await 之前更新，因此慢客户端不会丢失已接受文本，也不会在 worker join 后追加迟到字节。不调用 vendor abort API。`CleanupFailed` 仍为 Err，不能伪造取消或空闲；根 Agent shutdown、owner abort、timeout、receiver closed 保留独立原因。

取消 assistant 可显式保存到历史，stop_reason 留在 payload 元数据而非正文，但不能放入 infer 上下文（含 Inject 嵌套）。AbortSystem 先于 MCL 执行及推理受理；共享事件由各消费者独立观察，不靠 take reply 独占通知。

## 介绍与职责

`InferencePlugin` 执行一次模型请求：共享 `AgentMessage` 快照和选定 `CallableResource` 工具定义快照进入，完整 `InferenceResponse` 经 oneshot 返回。它不创建 Agent、不写历史、不执行工具、不自动重试、续写或压缩；命令收口由 MCL 负责；assistant 是否保存由调用方显式发出 history_append 决定，推理成功不触发 message 写入。压缩等调用方也使用同一个入口，自行解释响应。

全局只有一份 `InferenceConfig`，包含模型路由、配置路径、共享 HTTP client、调度计划及重载关联。没有项目路由覆盖、在途推理表、输出路由、固定异步 System 或业务结果转发 System。插件仅安装 `inference_system`，处理统一请求、重载请求和共享重载输出。

## 安装

需要 `RuntimePlugin`、`AsyncRuntimePlugin` 和已为相同计划注册的通用闭包执行设施。默认计划为 `PRE_UPDATE`；更换计划时也要注册该计划的 closure system。AgentDriverLoaderPlugin 不再是插件依赖。

```rust
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin};
use core_plugin::App;
use inference_plugin::InferencePlugin;

let mut app = App::new();
app.add_plugin(RuntimePlugin::default())
    .add_plugin(ClosurePlugin)
    .add_closure_system(RuntimePlugin::PRE_UPDATE)
    .add_plugin(AsyncRuntimePlugin)
    .add_plugin(InferencePlugin::default()
        .with_config_path("/etc/margatroid/models.toml"));
```

安装时有界读取并完整编译配置，失败或重复安装拒绝。`with_schedule` 和 `with_config_path` 分别配置计划和唯一配置路径；没有字符串协议工厂扩展入口。安装时通过一次 no-op async closure 验证通用设施。

## 全局模型路由与格式

默认配置为 `~/.margatroid/models.toml`。provider必填，仅接受 `openai`、`anthropic`、`deepseek`、`openrouter`；api_type必填，仅接受上述三种格式。未知字段也拒绝。例如OpenRouter路由：

```toml
[[models]]
id = "deepseek-v4-flash"
model = "deepseek/deepseek-v4-flash-latest"
provider = "openrouter"
base_url = "https://openrouter.ai/api/v1"
api_key = "replace-with-your-key"
api_type = "chat_completions"
thinking = "enabled"
reasoning_effort = "high"
context_window = "200k"
```

`id` 是 Agent.info.model.model 选择的逻辑键；`model` 是发送给Provider的上游模型值；provider是实际执行厂商，不从模型品牌、URL或格式推断。配置必须非空、ID唯一、必需字段非空、URL使用http/https。全部路由与安全catalog完整编译成功才发布；catalog只包含canonical provider、格式、模型和窗口等非敏感元数据，不含URL、密钥或adapter。

`base_url` 是用户指定的API前缀，去掉末尾斜杠后仅追加 `/chat/completions`、`/messages` 或 `/responses`。没有DeepSeek主机名或路径猜测；若兼容Messages端点要求 `/anthropic/v1/messages`，用户应将前缀设为 `https://example.test/anthropic/v1`。Messages使用x-api-key和anthropic-version，其他格式使用Bearer鉴权。

`context_window` 使用不区分大小写的无符号整数后缀 `k/m/b/t`，分别乘以 1000、1000000、1000000000、1000000000000，必须大于零且不溢出；缺省为 `1m`。它是路由信息，不是假称精确 token 估算，也不是输出长度限制。采样参数直接复制已存在的 `Agent.info.model`，不会每次重新读取 AgentDriver。

### Chat Completions

使用 `chat/completions` 流式请求，工具名直接为资源 alias，开启 `stream_options.include_usage`。累积器支持 CRLF、SSE 多 data 行和跨 UTF-8 字节分片，分别累积正文、reasoning 及工具调用 index 对应的 ID/name/arguments 片段。工具 index、ID、名称及参数均有界；最终检查唯一 ID、非空名称、JSON object 参数，以及名称是否属于本次可见工具。

usage 尾帧即使没有 choices 也处理；统一 input/output 与 prompt/completion 计数，支持缓存命中字段。必须观察 choice、真实停止原因及 `[DONE]` 才成功，半流或非法分片不能当正常截断。保留 `Length`、`ContentFilter` 和其他 Provider 停止原因，不因工具调用覆盖停止原因；完整工具响应没有正文也可成功。

### DeepSeek与OpenRouter选项

DeepSeek的thinking/reasoning_effort仅允许chat_completions；messages/responses设置任一选项会在路由编译时拒绝。`thinking = "disabled"` 会显式编码 `{type:"disabled"}`，不能省略后误启用模型默认思考。`enabled` 必须同时设置 `reasoning_effort = "high"` 或 `"max"`；其他组合在路由编译阶段拒绝。未指定 thinking 时不发送该配置字段，但历史 assistant 按思考协议回传 `reasoning_content`，缺失时补空串。显式 disabled 时，工具调用历史仍保留必要 reasoning 字段。

OpenRouter同样只在chat_completions允许这些选项：enabled必须搭配high（不支持max），映射为 `reasoning: { enabled: true, effort: "high" }`；disabled映射为 `reasoning: { enabled: false }`。不发送DeepSeek的thinking/reasoning_effort字段。OpenAI/Anthropic拒绝这两项选项。

响应接受 `reasoning_content` 和兼容的 `reasoning`，正文与思考分别推送。原生 `prompt_cache_hit_tokens` 及兼容 cached-token details 都映射到共享 usage。

### Messages、Responses与reasoning replay

Messages将System提取为system文本块，工具调用/结果编码为tool_use/tool_result；必须提供max_output_tokens，不编造默认max_tokens。SSE验证message_start、索引块的start/delta/stop、message_delta真实停止原因和message_stop，拒绝错序、重复、未闭合或残缺事件。

Responses每次发送完整本地上下文，`store: false` 且 `include: ["reasoning.encrypted_content"]`；不依赖conversation或previous_response_id。函数调用与输出以call_id关联；不支持stop序列时显式拒绝。SSE按output item/content part及格式自己的终态检查完成，不把[DONE]当作所有格式的结束条件。

Assistant新增可选 `replay: Option<serde_json::Value>`，旧消息缺字段默认为None。它是领域层不解释的API-tagged JSON信封：`{ "api_format": "messages" | "responses", "items": [...] }`。Messages保留thinking的文本及signature、redacted_thinking的data；Responses保留reasoning item的id、summary及可用的content/encrypted_content。完整响应和显式历史/DTO往返保留它，显示reasoning不能代替它。不兼容格式、无效信封及缺少必要replay的历史reasoning在HTTP前拒绝；Chat Completions不接受外来replay，OpenAI chat也拒绝非空历史reasoning。User支持独立images，工具结果仍为String纯文本。

## 发起一次推理

请求、错误及重载事件位于共享 `types::inference_events`，响应与消息位于共享 `types::message_types`；插件重新导出公开请求和响应，不再有插件 events 模块。

```rust
use app_runtime_plugin::WorldEventExt;
use inference_plugin::InferenceRequest;
use margatroid_types::message_types::AgentMessage;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

let (sender, receiver) = oneshot::channel();
world.send_event(InferenceRequest {
    id: "request-1".into(),
    agent,
    messages: vec![AgentMessage::User { content: "Review this patch.".into(), images: vec![] }],
    tools: selected_callable_resources,
    turn: Some("turn-1".into()), // 当前活动轮次快照
    cancel: agent_scope.child_cancel(),
    reply: Arc::new(Mutex::new(Some(sender))),
});
// 异步调用方 await receiver，收到 Result<InferenceResponse, InferenceError>。
```

主线程先认领原 Sender 并转入私有回复槽，重复克隆请求不会重复执行。校验完整 Agent Entity、请求 ID 和取消状态；从 `Agent.info.model` 获取模型与采样参数。实际受理时再次核对当前 turn 与请求快照；None 或已中止旧轮次返回正常 cancelled 响应，不启动 HTTP；turn 不作为推理唯一 ID。请求取消令牌与活动作用域派生 child 同时监督，即使请求使用独立令牌也不能绕过 Agent 停止。

`tools: Vec<CallableResource>` 保存完整 ResourceID、选定 alias 及 `Arc<CallableDefinitionSnapshot>`。直接消费该快照的描述、schema 和资源种类，不按 ID 重取最新定义，也不重复 global alias query。归属校验要求目标 ResourceSet 中该 ID 存在、Agent.driver_dependencies 许可，以及 ImportBindings 中该 ID/TOOL kind 已导入；不要求旧 snapshot 与 reload 后的新 alias/定义一致。visible alias 必须唯一，不按重复 ID 静默去重。

tool 使用选定 schema；仅 tool 资源可作为 TOOL。空 tools 就是无工具，不回退默认集合。非法 schema、外部 `$ref` 或归属失败在 HTTP 提交前拒绝。

准备阶段仍调用旧 `normalize_messages` 作预检并丢弃其 `ProviderMessage` 投影，随后将原始拥有所有权的AgentMessage放入DomainRequest，由格式层再次规范化/编码，replay不会通过旧投影丢失。`Inject` 递归按序展开；System、user、assistant、工具结果及 error 保留真实角色，System 与 prompt 的真实 role 不降格为 user。保留 reasoning、工具 ID、参数和结果关联，历史 usage 与 stop_reason 不进入请求，cancelled assistant 拒绝作为上下文。检查工具调用/结果配对，空输入、孤立结果、重复调用和超限输入拒绝。history append 不接收 System 是独立约束，不缩窄 infer 输入。

MCL `infer` 参数为 MESSAGE array + TOOL array；手工工具调用也先构造恰好一条 assistant MESSAGE，再传 visible TOOL array，calls 从 `assistant.tool_calls` 提取。`ToolCall` 仍为 assistant 的纯结构字段，没有 TOOL_CALL Block array。metadata reload 不改变已取出的 Block/请求；新 GET alias 取得新 snapshot。脚本仍按选定路径 late read，metadata snapshot 不保证磁盘文件内容版本隔离。


## 执行监督与真实 cleanup

通用 async closure 调用 `execute_inference`。入口 spawn 独立 supervisor 并 await；入口丢弃仅通过私有 DropGuard 取消 owner token，supervisor 不随入口被丢弃。

supervisor 在共享 AgentExecutionScope 登记活动的 `mark_started` 后 spawn HTTP worker，分别监督 scope child、用户 turn_cancel、请求 cancel、owner cancel、回复 receiver 关闭及整体超时。正常结果显式 join worker；取消、超时或 receiver 关闭则显式 abort worker 并 await JoinHandle，证明 HTTP/SSE/增量发送 future 已退出释放。Provider panic 经 JoinError 转为关联错误。

随后有界发送终态，释放发送 future，才 `mark_completed` 并完成唯一 oneshot。终态阶段 owner abort 同样不会永久毒化作用域。未启动的提交失败 rollback；supervisor 自身异常退出且无法证明 cleanup 时，TaskGuard 标记 cleanup_failed 并保留活动，不以 Drop、日志或 Sender 关闭假称任务退出。

## WebUI 推流

准备阶段调用共享 `WorldClientExt::client_webui_senders()`：仅包含已注册、client_type 为 webui 的连接，发送器去重并忽略断开项。没有发送器仍正常推理；本次启动后新增连接不加入快照。不按 Workspace 隔离，不依赖 ClientPlugin Handler 或全局 config 的推流 target。

正文使用 `agent.message.delta`，思考使用 `agent.message.reasoning_delta`，字段为 `id`、Agent 逻辑地址 `agent`、不透明 `agent_instance`、递增 `sequence` 和 `content`。实例字符串由完整 Entity 的 Debug 格式生成，与 Protocol Agent 快照一致，仅供关联，不接受入站解码寻址。工具参数碎片不推送为可执行调用。

每连接并发发送超时为 100ms；慢或关闭连接从增量活动集合剔除，旁路失败不改变模型结果。最终向原快照发送：

```json
{"type":"agent.message.finished","id":"request-1","agent":"agent:demo/main","agent_instance":"opaque-instance","error":null}
```

失败时 `error` 为 `{ "kind": "ResponseIncomplete", "message": "..." }`。终态只是显示旁路，oneshot 是唯一权威结果。已显示部分增量后失败不撤回内容，也不自动重试。

## 全局重载

```rust
use app_runtime_plugin::WorldEventExt;
use inference_plugin::ReloadModelRoutes;
world.send_event(ReloadModelRoutes { id: "reload-1".into() });
```

一个重载在途时返回 `ConfigBusy`。后台有界读取并解析 TOML，共享 `InferenceReloadOutput { id, result: Result<String, InferenceError> }` 携带原始配置，不携带插件 Adapter。原始字符串包含凭据，只用于内部编译，不发客户端、不记日志。主线程核对关联并完整编译vendor/format适配器，捕获编译 panic，成功整体替换 routes；任一失败保持旧配置，清除标记并发送 `ReloadModelRoutesResult { id, result: Result<usize, InferenceError> }`。ReloadGuard 保证闭包丢弃仍投递带原 ID 的失败。

重载期间新推理继续使用旧配置，已启动推理使用自己的协议快照。不监听文件变化，不接受项目路径覆盖。

## 上限、错误与扩展

实际固定上限：配置 1MiB；单消息文本序列化 4MiB、总文本输入 16MiB、含base64消息上下文及最终HTTP请求体32MiB、4096 条、inject 深度 32；工具 256 个、名称 64 字节、描述 8KiB；stop 64 个且每项 256 字节；错误 body 16KiB；响应总量 32MiB；单 SSE buffer/event 4MiB；单项正文/reasoning/工具参数 4MiB；整体请求 180 秒；receiver 关闭每 20ms 检查。不存在可配置的 InferenceLimits 结构体。

错误共享 `kind/message`，文本按 UTF-8 边界限制到 512 字节；HTTP 状态由 `with_status` 写入 message，没有独立 status 字段。HTTP 错误正文优先提取常见 JSON 字段并单行化；传输错误只使用安全 URL origin 和分类，不复制可能带凭据的底层错误链，不记录 Authorization、请求正文或完整 prompt。

扩展格式时修改具体请求、ApiRequest分支及对应SSE编解码；扩展厂商时修改Vendor、显式支持矩阵及vendors处理。`ProviderAdapter::build_request` 消费拥有所有权的DomainRequest并返回ProviderHttpRequest；每个响应创建独立ProviderResponseAccumulator。push返回有序增量，finish验证该格式完整终止后返回完整响应与尾部增量。保留公开trait及ErasedProviderAdapter供调度，但没有注册工厂或旧ProviderInput借用接口。

完整六板块契约见 [DESIGN.md](<DESIGN.md>)。回归覆盖协议 SSE/DeepSeek、资源可见性、半流、取消、receiver 关闭、panic、HTTP/终态 owner abort，以及重载忙碌与旧配置保留。
