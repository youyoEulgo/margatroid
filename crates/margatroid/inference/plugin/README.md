# inference_plugin

独立单次推理 plugin，组合 inference_core、Agent、Client、Tools记录与 address core，使用 mecs 运行设施。入口是 `InferencePlugin::open(&Path)`，有界读取并完整校验既有 models.toml；`with_schedule` 设置循环 schedule，默认 update，仅允许UPDATE或POST_UPDATE，拒绝PRE_UPDATE及无法证明中止先行的自定义schedule。安装前需要 RuntimePlugin 与 AsyncRuntimePlugin，拒绝重复安装，不要求其他领域plugin；异步退出通知需要WorldClosureSender，装配应提供目标schedule的ClosurePlugin处理入口。

配置保留 `[[models]]` 的 id、model、provider、api_type、base_url、api_key、context_window、thinking、reasoning_effort 字段。provider 与 api_type 明确匹配以下七种组合，不猜测 URL 或模型类型：

| provider | api_type |
| --- | --- |
| openai | chat_completions、responses |
| anthropic | messages |
| deepseek | chat_completions、messages、responses |
| openrouter | chat_completions |

context_window 接受 k/m/b/t 后缀，默认 1m。未知字段、重复路由、不兼容选项、非法 URL 或鉴权字段直接失败。凭据不写日志或前端错误，HTTP 错误不暴露完整请求、鉴权头或敏感 URL。

InitInference 在存活 Agent 上同步初始化 ActiveInference，不覆盖已有句柄。请求受理验证完整Entity、Starting/Running及并发推理句柄；显式Some(turn)额外要求Running和当前turn匹配，省略None自由调用，显式turn不匹配时返回空assistant的Completed，不创建HTTP或监督任务。当前Tools定义、共享Clients、Address、Agent.instance与本次生成租约均固定为owned材料，不跨await借World；文件型模型路由由监督重读来源，显式内存模式才用受理时路由。无需DriverLuaVm或旧AgentWaitSlot，完成后复核完整实例并唤醒Agent.wake_slot，不改变Agent状态。当前句柄真实结束后可被下一次受理替换。

InferenceRequest没有tools参数。每次推理遍历当前Agent.Tools，以alias、description、input_schema构造本次模型定义，不重读文件，不由settings或Driver筛选；Tools为空则无工具。导入后的文件metadata变化不会隐式更新记录。输入使用 inference_core 的独立消息类型，校验角色、工具调用配对、参数、图片以及请求总预算。

DomainRequest通过TryFrom转换为ChatCompletionsRequest、MessagesRequest或ResponsesRequest；EncodeInput只在插件内提供目标能力。ApiAdapter按七个明确组合分派至OpenAi<T>、Anthropic<T>、DeepSeek<T>或OpenRouter<T>的具体process；T必须是具体请求类型，各支持组合分别实现process，不存在通用VendorAdapter对JSON做厂商修补。公共传输辅助只负责端点、头和有类型请求序列化。响应解码器随构造出的HTTP请求携带，HTTP worker不再根据路由字符串另选格式。

Messages要求max_output_tokens，Responses拒绝stop，提交完整本地上下文且store=false。内部消息只保留一份reasoning、正文和工具关联，没有api_format、签名原生块或加密重放字段。目标支持独立历史reasoning字段时在格式转换中映射；不支持时保留正文与工具，不把思考塞入正文或伪造签名。只有reasoning而无正文或工具、目标又无法表达时明确拒绝转换。响应可读思考归一为reasoning，签名和加密协议材料不进入共享结果或累积器。内部用户及工具消息图片集合键为data，图片项使用media_type与base64 data；验证标准base64、数量、解码字节和magic。Chat在工具结果整批齐备后追加user图片投影，Messages/Responses使用原生布局。

HTTP/SSE worker 只输出到容量 32 的 ProviderFragment 通道，不向前端发送。解码器维护字节缓冲、协议索引、usage 计数及 Responses 完整性摘要，不保留另一份最终正文或工具参数。Content、Reasoning、ToolCallDelta、Usage、Finished 全部经同一个接收出口进入唯一 ResponseAccumulator。

接收循环使用 biased select 优先取消，每轮只取一个分片，进入 StreamGate 短锁后复检关闭状态。只有接受的片段才能更新正文并获得 sequence。stop_stream 同步关闭门，InferenceHandle.cancel 先冻结再发取消；receiver 随即关闭并丢弃未接受 backlog。中止回复使用completed，只保留已接受正文和思考，清除工具调用、usage，不填充取消文案。

推流使用agent.message.chunk.sync，通道每取出一个片段先提交唯一累积器，再发送其完整content/reasoning及快照sequence。终态仅使用一次agent.message.full.sync，generation为推理关联、history_sequence=null，Inference不写历史也不伪造sequence。保存带request_id的assistant/error后记录监控不重发full，持久序号由history查询取得。已交付错误后的清理超预算只记结构化诊断并继续真实监督，不再次发终态或回复oneshot。发送在接受门锁外按当次共享Clients筛选webui，慢连接取消且不阻塞推理，后连接可以接收后续累计快照。

内容结果与清理完成分别处理：已确认的provider或流错误可先回复，但监督始终等待HTTP worker真实join，不以冻结代替清理。清理超过预算仍等待真实结束并报告 CleanupFailed；Timeout、ReplyClosed、协议错误等保留各自分类。HTTP worker 退出后不能用独立正文补写结果。不改网络中止事件路径，不写历史，不执行工具或自动重试。

预算包括配置 1MiB、请求/响应各 128MiB、SSE 单事件 4MiB、正文与工具参数及图片上限。单消息图片最多 8 张，每张解码 5MiB，合计 20MiB。整体执行默认 180 秒，工具定义准备计入期限。连接超时为 30 秒；等待响应头受整体剩余期限约束，流开始后每次读取的空闲上限为 60 秒，不把该空闲上限用于首响应等待。

装配层集成验证通过本地 HTTP 检查七种组合、鉴权、端点、图片、已登记工具定义及 Entity generation，并通过真实DTO/WebSocket注册、Agent地址投影及推理流核对目标关联。内部并发测试物理上位于 tests/internal：使用真实 HTTP 填满有界通道，验证未接受 backlog 不进入冻结累积器，并通过测试专用清理门验证同步冻结先于 worker join、oneshot 等待真实 join。另覆盖严格终态、UTF-8 跨字节、签名及加密材料不进入内部消息、跨API保留正文与工具、完整工具调用及 SSE 预算。

工具目录不按条目数或单条描述长度截断，统一受HTTP请求总字节预算约束。真实HTTP回归覆盖1024个工具、每条16KiB描述，在三种模型格式中逐条核对完整名称、描述及schema。

受理阶段固定Tools定义及输入材料，文件模型来源在监督中重读、校验后编码；显式内存模式可在受理时构造请求。未知工具名只要响应结构合法就保留，不在推理结果层按工具清单拒绝；当前Tools存在性和schema校验由工具调用受理负责。内部取消信号和错误类别保留，正常中止向调用方返回completed，不影响真实清理要求。

清理通知不通过send_event_after循环等待。尚未结束的实际句柄由独立异步wait_finished等待，再提交一次World闭包，核对Agent.instance、request_id与同一生成租约后唤醒。模型公共投影从core的ModelObserverFactory取得通用回调，client_core只在plugin负责投递。本文描述源码实现，不表示运行中的daemon已更新。

普通InferenceHandle.cancel只冻结接受门并请求取消，保留共享来源租约，使同generation的终态full能够交付。retire_source单独用于Agent真正关闭或来源替换，淘汰旧实例的已入队帧；调用方随后仍需cancel并等待真实清理。两者不可混同，回合中止不能提前退休终态来源。共享lease取消回归验证冻结文本、一次full、真实join及迟到文本拒绝。
