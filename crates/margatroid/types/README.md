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

let driver = ResourceId::parse("driver:local/coder:latest")?;
assert_eq!(driver.resource_type(), "driver");
```

资源ID固定使用 `type:scope/name:tag`。省略tag时规范化为 `latest`。Agent、Workspace、AgentDriver、
Prompt、Sandbox和Tool都使用同一种身份格式。`ResourceId`是跨Plugin查找资源的统一地址；
`core_plugin::Entity`只作为当前World内的运行时句柄，不进入协议、日志或Memory。

Compose使用 `WorkspaceDefinition` 和 `WorkspaceAgentDefinition` 把配置文件编译成进程内静态业务
输入。它们不包含运行时Entity，也不负责加载驱动、打开Memory或创建AgentInstance。

## 领域推理请求分层（已实现）

共享层单独以 `inference_request_types.rs` 保存纯值 `DomainRequest { messages: Vec<AgentMessage>, tools: Vec<ToolDefinition>, config: DomainRequestConfig }`。`DomainRequestConfig` 只包含上游模型名、temperature、max_output_tokens、top_p、stop；不包含厂商扩展、URL、密钥、HTTP Client、Entity、取消或回复通道。Default 是空请求，不是有效的默认模型路由。

`InferenceRequest` 继续表示带运行期关联和 oneshot 的执行事件，不能与纯值请求混为一体。API 格式、三种具体请求、`ApiRequest`、厂商处理器及转换错误只在 `inference_plugin::types` 定义；共享层不依赖推理插件。可失败转换采用 `TryFrom` / `try_into()?`，不为方便而丢弃消息信息。三个具体格式转换及vendor处理已经实现；共享层不包含具体HTTP格式。

## reasoning replay（已实现）

Assistant包含可选 `replay: Option<serde_json::Value>`；旧消息缺字段默认None。共享层将其视为不透明纯JSON：`{ "api_format": "messages" | "responses", "items": [...] }`。Messages保存signed thinking的文本/signature及redacted_thinking.data；Responses保存reasoning item的id、summary和可用的content/encrypted_content。DTO、持久MESSAGE和显式历史保留replay，不把它拼入正文，也不以可显示reasoning代替签名或加密元数据。格式边界拒绝不兼容/无效replay及无法无损表达的历史；Responses使用store=false及include reasoning.encrypted_content实现完整本地无状态重放。User和ToolCall结果均支持独立images数组，content保持String。

## 共享消息与执行边界

`AgentMessage` 是纯消息值，不是带 Entity 的路由事件；包含 Inject、System、User、Assistant、ToolCall 结果及 Error。`ToolCall` 输入只存在于 Assistant.tool_calls。资源可见性由 Agent 的显式导入绑定与 MESSAGE/TOOL/SANDBOX typed arrays 决定，不恢复旧 AgentFailure、邮箱或可见性集合。

## 用户轮次中止契约（已实现）

无 `get_turn`。Agent采用 Creating / Idle / Working / Stopping，对外同步为 `creating/idle/working/stopping`，同时发布 turn 与 status。启动成功进入 Idle、turn=None；`start_turn` 仅允许 Idle，原子创建唯一 turn 并置 Working，不覆盖已有轮次。`AbortAgentTurn` 验证完整Agent实例、workspace、Driver实例及expected_turn_id；受理后立即清空 turn，但保持 Working，只取消匹配活动的独立 `turn_cancel`，不关闭根作用域。正常结果链的 `finish_turn` 核对当前Driver关联与真实清理完成后才置 Idle；Idle重复完成幂等，Working且turn=None并不代表完成，迟到旧完成不能覆盖新轮次。用户轮次中止不等于根 Agent shutdown、owner abort、timeout 或 receiver closed。

Assistant 的 `stop_reason: Option<StopReason>` 使用serde默认值兼容旧文件缺字段为None；prompt等非模型生成消息使用None，每个实际推理输出使用Some，覆盖所有推理原因，包含 `StopReason::Cancelled`（序列化 `cancelled`）。兼容保留 `InferenceResponse.stop_reason`，由同一权威映射构造，必须与Assistant中的Some值相同。用户轮次中止且真实清理成功返回 Ok assistant：保留已接受的部分 `content` 与 `reasoning`（二者皆空时均为 `None`，不填充提示正文），`replay=None`、`tool_calls=[]`、`usage=None`、`stop_reason=Some(Cancelled)`。

取消工具且真实清理成功返回 Ok ToolCall 结果：`failed=true`、`content="Tool call aborted."`，保留调用 ID、名称及已知 resource_id；未启动项同用该正文。批次每个调用均有一个结果，保留已完成结果，不虚构部分成功，不撤销已发生副作用。`CleanupFailed` 仍为 Err，保留活动，不宣称空闲。

AbortAgentTurn控制系统注册在 `RuntimePlugin::PRE_UPDATE`，先于UPDATE中的 MCL 实际执行与 inference/tool admission；自定义调度必须保留这个先后关系。共享中止事件允许多个消费者独立观察，不靠 take reply 独占通知；实际受理重新核对Working及非空匹配的请求 turn，turn=None 或已中止旧轮次返回正常取消结果，不启动执行。

历史仅由显式 MCL `history_append` 保存。Assistant stop_reason 存在历史 payload 元数据，不混入正文；cancelled assistant 可入历史但不可进入 infer 上下文（包括 Inject 嵌套）。领域请求、结果与错误保持各自 oneshot/共享事件关联，不恢复 AgentFailure 路由。

## Workspace 成员启动失败隔离契约

Workspace 自身定义校验、创建或执行线程创建失败才返回 StartWorkspaceResult::Err；创建前失败不占用新地址，重复地址保留旧实例并报告其 ID、状态、成功成员数及失败成员数，不覆盖或自动附着。初始驱动和成员请求全部结束后，即使全部成员失败也进入 Running 并返回 Ok，成员失败不得关闭提交门。

Workspace 保存 member_failures: BTreeMap<ResourceID, WorkspaceMemberFailure>；失败值包含 driver 与 error，工作区身份由所属组件提供。这是持久到该工作区停止的失败诊断，不是重复 Agent 生命周期表。每个失败成员产生 WorkspaceMemberStartFailed，携带 workspace Entity、workspace_id、agent、driver、error；共享驱动失败按成员展开，动态创建失败同样记录并通知，成功加入清除该成员旧失败。Stopping 中已受理动态创建失败只结束等待，不污染健康工作区的停止结果。

WorkspaceInfoDto 新增 member_failures: Vec<WorkspaceMemberFailureDto>，各项 agent、driver 为 ResourceIDDto，error 为 String；serde 缺省为空、空列表省略。workspace.started、state.sync、state.query 均投影该唯一失败源。DTO 按配置 logs 目标逐成员发送包含工作区、成员、驱动和原始错误的日志。manager 不可用时路由返回明确 manager unavailable 诊断，不暗中切换成员。客户端收到带失败的 workspace.started 仍保持连接。

## 用户及工具结果图像契约

User与ToolCall执行结果保持 `content: String`，增加 `images: Vec<UserImage>`；缺少images反序列化为 `[]`，空数组序列化省略。`UserImage { media_type: String, data: String }` 中data是规范标准base64（含必要padding），不是data URL；允许image/png、image/jpeg、image/gif、image/webp。每条User或ToolCall结果最多8张，每张解码后最多5 MiB，总计最多20 MiB。共享公开函数 `validate_user_images(images: &[UserImage]) -> Result<(), String>` 在解码前检查数量、编码长度及解码长度上界，再严格验证base64及与MIME一致的PNG/JPEG/GIF/WebP魔数，不执行完整图像解码。错误只含索引、限制和原因，不回显base64。公开serde辅助函数 `deserialize_user_images` 供共享消息及边界复用；领域User和ToolCall结果反序列化也调用共享验证，覆盖直接MCL/Lua/持久化入站；Protocol入站递归转换调用共享验证，出站原样保留images；MESSAGE、Lua及历史payload保留图像，不将base64拼入正文。协议原有1 MiB非图像消息限制独立保留，图像编码单独按上述限制计量。
