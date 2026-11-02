# inference_plugin

独立单次推理 plugin，组合 inference_core、Agent、Client、工具路径与 address core，使用 mecs 运行设施。入口是 `InferencePlugin::open(&Path)`，有界读取并完整校验既有 models.toml；`with_schedule` 设置循环 schedule，默认 update。安装前需要 RuntimePlugin 与 AsyncRuntimePlugin，拒绝重复安装，不要求领域 plugin 或 ClosurePlugin。

配置保留 `[[models]]` 的 id、model、provider、api_type、base_url、api_key、context_window、thinking、reasoning_effort 字段。provider 与 api_type 明确匹配以下七种组合，不猜测 URL 或模型类型：

| provider | api_type |
| --- | --- |
| openai | chat_completions、responses |
| anthropic | messages |
| deepseek | chat_completions、messages、responses |
| openrouter | chat_completions |

context_window 接受 k/m/b/t 后缀，默认 1m。未知字段、重复路由、不兼容选项、非法 URL 或鉴权字段直接失败。凭据不写日志或前端错误，HTTP 错误不暴露完整请求、鉴权头或敏感 URL。

InitInference 在存活 Agent 上同步初始化 ActiveInference，不覆盖已有句柄。请求受理验证完整 Entity、Working、当前 turn 和并发推理句柄，无有效 turn 返回正常 Cancelled。模型路由、工具路径、已注册 webui 发送器、Address 和 DriverLuaVm.instance.start_id 均取本次快照，不跨 await 借用 World。受理时要求 Driver VM 的 owner 匹配完整 Agent Entity 且 start_id 非空；缺失或不匹配返回 AgentMissing。该 start_id 与 DTO 对外公布的 agent_instance 一致，后续 VM 重启不改写在途推理的身份。当前句柄真实结束后可被下一次受理替换。

每次推理通过 blocking worker 从工具 alias 路径重新读取定义。删除或格式失败不会回退旧定义，不保留 ResourceID 或工具快照。输入使用 inference_core 的独立消息类型，校验角色、工具调用配对、参数、图片以及请求总预算。

DomainRequest通过TryFrom转换为ChatCompletionsRequest、MessagesRequest或ResponsesRequest；EncodeInput只在插件内提供目标能力。ApiAdapter按七个明确组合分派至OpenAi<T>、Anthropic<T>、DeepSeek<T>或OpenRouter<T>的具体process；T必须是具体请求类型，各支持组合分别实现process，不存在通用VendorAdapter对JSON做厂商修补。公共传输辅助只负责端点、头和有类型请求序列化。响应解码器随构造出的HTTP请求携带，HTTP worker不再根据路由字符串另选格式。

Messages要求max_output_tokens，Responses拒绝stop，提交完整本地上下文且store=false。同格式合法的Messages thinking签名、遮蔽内容和Responses encrypted_content完整回放；缺少replay或属于其它已知格式的历史，只转换正文与工具关联，不伪造专用思考块，也不修改本地历史。损坏或未知格式的已提供replay仍失败；本次响应缺失必要元数据仍严格报协议错误。用户及工具图片验证标准base64、数量、解码字节和magic。Chat在工具结果整批齐备后追加user图片投影，Messages/Responses使用原生布局。

HTTP/SSE worker 只输出到容量 32 的 ProviderFragment 通道，不向前端发送。解码器维护字节缓冲、协议索引、usage 计数及 Responses 完整性摘要，不保留另一份最终正文或工具参数。Content、Reasoning、ToolCallDelta、Replay、Usage、Finished 全部经同一个接收出口进入唯一 ResponseAccumulator。

接收循环使用 biased select 优先取消，每轮只取一个分片，进入 StreamGate 短锁后复检关闭状态。只有接受的片段才能更新正文并获得 sequence。stop_stream 同步关闭门，InferenceHandle.cancel 先冻结再发取消；receiver 随即关闭并丢弃未接受 backlog。Cancelled 只保留已接受正文和思考，清除工具调用、replay、usage，不填充取消文案。

前端维持 agent.message.delta、agent.message.reasoning_delta、agent.message.finished，均携带 request id、Address 和 agent_instance。两种 delta 使用 content 文本字段及递增 sequence；finished 携带 error，不要求 sequence。发送在接受门锁外使用 mecs 连接已有的有界队列，满或关闭的连接从本次出口剔除，不阻塞接收循环，不改变后端结果。后续新连接不加入本次快照。

监督任务请求 HTTP worker 退出并等待真实 join 后才完成原 oneshot，不以冻结代替清理。清理超过预算仍等待真实结束并报告 CleanupFailed；Timeout、ReplyClosed、协议错误等保留各自分类。HTTP worker 退出后不能用独立正文补写结果。不改网络中止事件路径，不写历史，不执行工具或自动重试。

预算包括配置 1MiB、请求/响应各 32MiB、SSE 单事件 4MiB、正文与工具参数及图片上限。单消息图片最多 8 张，每张解码 5MiB，合计 20MiB。整体执行默认 180 秒，工具定义准备计入期限。连接超时为 30 秒；等待响应头受整体剩余期限约束，流开始后每次读取的空闲上限为 60 秒，不把该空闲上限用于首响应等待。

装配层集成验证通过本地 HTTP 检查七种组合、鉴权、端点、图片、fresh 工具及 Entity generation，并通过真实 DTO/WebSocket 注册、state.sync 广告和推理流核对 VM 重启前后的 agent_instance。内部并发测试物理上位于 tests/internal：使用真实 HTTP 填满有界通道，验证未接受 backlog 不进入冻结累积器，并通过测试专用清理门验证同步冻结先于 worker join、oneshot 等待真实 join。另覆盖严格终态、UTF-8 跨字节、签名和加密 replay、完整工具调用及 SSE 预算。
