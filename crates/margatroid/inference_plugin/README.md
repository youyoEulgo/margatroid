# InferencePlugin

显式message历史策略已实现：推理和工具调用只返回结果，所有message历史仅由显式MCL history_append控制。MCL指令审计仍自动记录，审计payload中的消息不作为独立message记录。示例Driver对需要保留的结果另发显式追加指令。


## 介绍与职责

`InferencePlugin` 执行一次模型请求：共享 `AgentMessage` 快照和选定 `CallableResource` 工具定义快照进入，完整 `InferenceResponse` 经 oneshot 返回。它不创建 Agent、不写历史、不执行工具、不自动重试、续写或压缩；命令收口由 MCL 负责；assistant 是否保存由调用方显式发出 history_append 决定，推理成功不触发 message 写入。压缩等调用方也使用同一个入口，自行解释响应。

全局只有一份 `InferenceConfig`，包含模型路由、配置路径、协议工厂、共享 HTTP client、调度计划及重载关联。没有项目路由覆盖、在途推理表、输出路由、固定异步 System 或业务结果转发 System。插件仅安装 `inference_system`，处理统一请求、重载请求和共享重载输出。

## 安装

需要 `RuntimePlugin`、`AsyncRuntimePlugin` 和已为相同计划注册的通用闭包执行设施。默认计划为 `PRE_UPDATE`；更换计划时也要注册该计划的 closure system。AgentImageLoaderPlugin 不再是插件依赖。

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

安装时有界读取并完整编译配置，失败或重复安装拒绝。`with_schedule`、`with_config_path` 和 `with_api_type` 可配置计划、唯一配置路径和额外协议工厂；空或重复协议名拒绝。安装时通过一次 no-op async closure 验证通用设施。

## 全局模型路由与协议

默认配置为 `~/.margatroid/models.toml`：

```toml
[[models]]
id = "deepseek-v4-flash"
model = "deepseek/deepseek-v4-flash-latest"
provider = "openrouter"
base_url = "https://openrouter.ai/api/v1"
api_key = "replace-with-your-key"
api_type = "deepseek"
thinking = "enabled"
reasoning_effort = "high"
context_window = "200k"
```

`id` 是 Agent.info.model.model 选择的逻辑键；`model` 是发送给 Provider 的模型值，两者不必相同。`provider` 仅为可选元数据，`api_type` 显式选择协议，不根据 URL 猜测。配置必须非空、ID 唯一、必需字段非空、URL 使用 http/https，所有条目完整编译成功才能发布。

`context_window` 使用不区分大小写的无符号整数后缀 `k/m/b/t`，分别乘以 1000、1000000、1000000000、1000000000000，必须大于零且不溢出；缺省为 `1m`。它是路由信息，不是假称精确 token 估算，也不是输出长度限制。采样参数直接复制已存在的 `Agent.info.model`，不会每次重新读取 AgentImage。

### OpenAI

使用 `chat/completions` 流式请求，工具名直接为资源 alias，开启 `stream_options.include_usage`。累积器支持 CRLF、SSE 多 data 行和跨 UTF-8 字节分片，分别累积正文、reasoning 及工具调用 index 对应的 ID/name/arguments 片段。工具 index、ID、名称及参数均有界；最终检查唯一 ID、非空名称、JSON object 参数，以及名称是否属于本次可见工具。

usage 尾帧即使没有 choices 也处理；统一 input/output 与 prompt/completion 计数，支持缓存命中字段。必须观察 choice、真实停止原因及 `[DONE]` 才成功，半流或非法分片不能当正常截断。保留 `Length`、`ContentFilter` 和其他 Provider 停止原因，不因工具调用覆盖停止原因；完整工具响应没有正文也可成功。

### DeepSeek

`thinking = "disabled"` 会显式编码 `{type:"disabled"}`，不能省略后误启用模型默认思考。`enabled` 必须同时设置 `reasoning_effort = "high"` 或 `"max"`；其他组合在路由编译阶段拒绝。未指定 thinking 时不发送该配置字段，但历史 assistant 按思考协议回传 `reasoning_content`，缺失时补空串。显式 disabled 时，工具调用历史仍保留必要 reasoning 字段。

响应接受 `reasoning_content` 和兼容的 `reasoning`，正文与思考分别推送。原生 `prompt_cache_hit_tokens` 及兼容 cached-token details 都映射到共享 usage。

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
    messages: vec![AgentMessage::User { content: "Review this patch.".into() }],
    tools: selected_callable_resources,
    turn: None,
    cancel: agent_scope.child_cancel(),
    reply: Arc::new(Mutex::new(Some(sender))),
});
// 异步调用方 await receiver，收到 Result<InferenceResponse, InferenceError>。
```

主线程先认领原 Sender 并转入私有回复槽，重复克隆请求不会重复执行。校验完整 Agent Entity、请求 ID 和取消状态；从 `Agent.info.model` 获取模型与采样参数。`turn` 可为空，不作为推理唯一 ID。请求取消令牌与活动作用域派生 child 同时监督，即使请求使用独立令牌也不能绕过 Agent 停止。

`tools: Vec<CallableResource>` 保存完整 ResourceID、选定 alias 及 `Arc<CallableDefinitionSnapshot>`。直接消费该快照的描述、schema 和资源种类，不按 ID 重取最新定义，也不重复 global alias query。归属校验要求目标 ResourceSet 中该 ID 存在、Agent.image_dependencies 许可，以及 ImportBindings 中该 ID/TOOL kind 已导入；不要求旧 snapshot 与 reload 后的新 alias/定义一致。visible alias 必须唯一，不按重复 ID 静默去重。

tool 使用选定 schema；skill 固定 `{ "type":"object", "properties":{}, "additionalProperties":false }`，只允许 `{}`。Hook 可消费自身 description/schema 生成模型定义，但 ToolPlugin 执行明确返回 `UnsupportedHook`，不代表 Driver 触发器路线已闭合。prompt/sandbox 不能冒充 TOOL；空 tools 就是无工具，不回退默认集合。非法 schema、外部 `$ref` 或归属失败在 HTTP 提交前拒绝。

`Inject` 递归按序展开；System、user、assistant、工具结果及 error 转为公开 `ProviderMessage`，System 与 prompt 的真实 role 不降格为 user。保留 reasoning、工具 ID、参数和结果关联，历史 usage 不进入请求。检查工具调用/结果配对，空输入、孤立结果、重复调用和超限输入拒绝。history append 不接收 System 是独立约束，不缩窄 infer 输入。

MCL `infer` 参数为 MESSAGE array + TOOL array；手工工具调用也先构造恰好一条 assistant MESSAGE，再传 visible TOOL array，calls 从 `assistant.tool_calls` 提取。`ToolCall` 仍为 assistant 的纯结构字段，没有 TOOL_CALL Block array。metadata reload 不改变已取出的 Block/请求；新 GET alias 取得新 snapshot。脚本和 Skill 正文仍按选定路径 late read，metadata snapshot 不保证磁盘文件内容版本隔离。


## 执行监督与真实 cleanup

通用 async closure 调用 `execute_inference`。入口 spawn 独立 supervisor 并 await；入口丢弃仅通过私有 DropGuard 取消 owner token，supervisor 不随入口被丢弃。

supervisor 在共享 AgentExecutionScope 登记活动的 `mark_started` 后 spawn HTTP worker，合并 scope child、请求 cancel、owner cancel、回复 receiver 关闭及整体超时。正常结果显式 join worker；取消、超时或 receiver 关闭则显式 abort worker 并 await JoinHandle，证明 HTTP/SSE/增量发送 future 已退出释放。Provider panic 经 JoinError 转为关联错误。

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

一个重载在途时返回 `ConfigBusy`。后台有界读取并解析 TOML，共享 `InferenceReloadOutput { id, result: Result<String, InferenceError> }` 携带原始配置，不携带插件 Adapter。原始字符串包含凭据，只用于内部编译，不发客户端、不记日志。主线程核对关联并完整编译协议工厂，捕获工厂 panic，成功整体替换 routes；任一失败保持旧配置，清除标记并发送 `ReloadModelRoutesResult { id, result: Result<usize, InferenceError> }`。ReloadGuard 保证闭包丢弃仍投递带原 ID 的失败。

重载期间新推理继续使用旧配置，已启动推理使用自己的协议快照。不监听文件变化，不接受项目路径覆盖。

## 上限、错误与扩展

实际固定上限：配置 1MiB；单消息序列化 4MiB、总输入 16MiB、4096 条、inject 深度 32；工具 256 个、名称 64 字节、描述 8KiB；stop 64 个且每项 256 字节；错误 body 16KiB；响应总量 32MiB；单 SSE buffer/event 4MiB；单项正文/reasoning/工具参数 4MiB；整体请求 180 秒；receiver 关闭每 20ms 检查。不存在可配置的 InferenceLimits 结构体。

错误共享 `kind/message`，文本按 UTF-8 边界限制到 512 字节；HTTP 状态由 `with_status` 写入 message，没有独立 status 字段。HTTP 错误正文优先提取常见 JSON 字段并单行化；传输错误只使用安全 URL origin 和分类，不复制可能带凭据的底层错误链，不记录 Authorization、请求正文或完整 prompt。

新协议通过 `with_api_type` 注册 `ProviderAdapterFactory`，工厂从借用的 `ProviderRouteInput` 构建 Adapter；Adapter 将借用的 `ProviderInput` 编码为拥有所有权的 `ProviderHttpRequest`，为每个响应创建 `ProviderResponseAccumulator`。累积器 `push` 返回有序增量，`finish` 返回完整响应和尾部增量，必须验证协议完成。公开 trait、路由和协议辅助值当前保留在插件 types 模块，共享消息、usage、StopReason 和错误不反向依赖插件。

完整六板块契约见 [DESIGN.md](<DESIGN.md>)。回归覆盖协议 SSE/DeepSeek、资源可见性、半流、取消、receiver 关闭、panic、HTTP/终态 owner abort，以及重载忙碌与旧配置保留。
