# Inference

inference_core定义推理请求、消息、结果、模型配置Resource及错误，只依赖mecs与通用库。inference_plugin组合Agent、Client、工具路径能力，执行模型请求；不依赖其他plugin。MCL负责消息转换、批次工具调用和显式历史，Inference不维护上下文、不执行工具、不写历史。

## 配置与参数

InferencePlugin::open读取models.toml并完整校验模型路由，build安装ModelRoutes及HTTP客户端。provider与api_type分别明确指定，不从URL或模型名推断。请求携带路由选择键及采样参数；受理时取得本次路由快照，后续配置变化不修改在途请求。配置重载归Margatroid整体机制，不增加独立推理重载事件。

| provider | api_type |
| --- | --- |
| openai | chat_completions、responses |
| anthropic | messages |
| deepseek | chat_completions、messages、responses |
| openrouter | chat_completions |

未知组合或不支持参数在HTTP前失败。base_url是API前缀，仅追加对应端点；Messages使用x-api-key及anthropic-version，其他格式使用Bearer。凭据、完整请求及敏感错误URL不进入日志或前端投影。非2xx响应有界读取错误正文，保留HTTP状态、上游message、type、code与request_id等诊断字段；剔除当前凭据，不转储完整响应。读取超时、断流、正文过大或无法解析时仍保留HTTP状态和读取情况，不将已知上游错误覆盖成笼统网络失败。

## 推理句柄初始化

Agent初始化时发送InitInference，init_inference_system路由至Handler，在Agent Entity挂载current=None的ActiveInference并通过原oneshot回复；已有组件不覆盖，不在初始化时提交推理。

ActiveInference归inference_core，单独保存当前InferenceHandle。受理推理时写入本次请求、turn和控制句柄，已有未结束推理时拒绝重叠请求。中止直接取得当前句柄，先关闭StreamGate再取消，不查找或遍历ToolTasks。

## 输入与工具

InferenceRequest携带完整Agent Entity、turn、消息、工具alias和oneshot。Handler认领唯一回复端，检查Working及当前turn，准备本次请求。无有效turn返回正常Cancelled响应，不启动HTTP。

推理消息由本领域独立定义，MCL在plugin层转换并展开Inject；core不引用消息、工具或Agent core。System保留原角色，取消Assistant不进入模型上下文，工具调用与结果必须正确配对。

工具参数是已导入alias数组，空数组代表无工具。准备本次请求时从Tools取得路径，读取最新描述与schema，拒绝重复alias或非法定义；不读取ResourceID快照、不缓存工具正文、不执行脚本。

## 请求适配

DomainRequest先通过TryFrom转换为ChatCompletionsRequest、MessagesRequest或ResponsesRequest；需要厂商能力时使用插件内EncodeInput。ApiAdapter只分派明确支持的组合，调用OpenAi<T>、Anthropic<T>、DeepSeek<T>或OpenRouter<T>对应的具体process实现，T是具体格式请求类型。参数处理使用有类型字段，不能先编码成通用JSON再按厂商分支补救。响应解码器由同一组合选定，逆向输出统一分片。Messages要求max_output_tokens，Responses不接受stop序列；不通过静默丢弃参数完成兼容。

Responses提交完整本地上下文，store=false，不依赖远端conversation或previous_response_id。历史消息的正文、工具调用和工具结果按目标格式转换；reasoning展示文本与协议专用replay分别保存。同格式且合法的Messages thinking签名/遮蔽内容、Responses reasoning/encrypted_content完整回放；没有replay或replay属于另一已知格式时，不向目标伪造专用思考块，不因此拒绝普通历史，也不修改本地消息。已提供的损坏replay或未知格式明确报错。本次响应若声明了思考回放信息，解码与累积仍严格验证完整性，不能用历史兼容规则掩盖响应元数据丢失。

用户和工具结果图片均支持PNG/JPEG/GIF/WebP标准base64，校验数量、字节和magic。Messages与Responses使用原生工具图片；Chat保留tool结果，整批结果齐备后追加格式层user图片消息，不修改本地消息或历史。

## 单一流出口

HTTP/SSE读取任务只向一个有界ProviderFragment通道写入，不直接向前端推流。格式解码器维护SSE缓冲及协议状态，不另外维护最终正文。正文、思考、工具参数、replay、usage和终态均通过该通道。

接收循环位于本次推理监督闭包，使用Tokio biased select!优先等待取消，每轮只读一个分片。追加前在StreamGate短锁内再次检查关闭状态；通过裁决才更新唯一ResponseAccumulator并分配sequence。只有已接受的正文或思考分片才能发送前端。

网络发送不持接受门锁，不因慢客户端阻塞接收循环。发送队列和缓冲有界；超过发送预算的连接停止接收本次流，不丢弃后端累积内容，也不把连接错误改成模型错误。发送器使用本次开始时已注册webui快照，后续连接不补入；无前端连接仍能完成推理。

正常结束时验证对应格式的真实终态；通道关闭本身不表示协议成功。Chat检查finish_reason和DONE，Messages检查message_stop及内容块闭合，Responses检查其真实终态。工具调用校验完整ID、alias和JSON参数，不能执行半截调用。

## 中止与结果

本轮保留原网络路径：Axum收到消息后发送通用网络事件，DTO处理后发送专属中止事件，下一帧处理控制。不改变Axum、mecs事件可见性或新增网络快速通道。

后端受理中止时同步关闭本次StreamGate，再触发任务取消。接受分片与关闭门共用同一短锁；门关闭后不再累积或准备发送任何新分片。receiver直接关闭并丢弃剩余队列，不继续排空，不等HTTP退出后才冻结。

前端点击中止立即拒绝本次request及实例的后续流，等待权威sync。后端受理存在网络及两帧延迟，因此sync仍可能校正少量内容；清理期间不再增加后端正文。

正常或用户中止的内容结果始终取自通道出口的ResponseAccumulator。上游任务退出后不能用另一份正文补写结果。中止冻结已接受的正文及思考，返回Cancelled，tool_calls为空，replay和usage为空；无内容保持空值，不填充取消文案。

文本冻结与任务清理分开。已确认的provider或流错误立即经oneshot交给MCL转换为error消息，不等待后台join；正常取消在清理预算内仍保留Cancelled语义，超预算及时返回CleanupFailed。原监督任务始终保留并等待真实join，ActiveInference不提前释放，下一次重叠推理返回可见错误。清理完成后发送绑定原Agent、VM实例和request_id的内部通知，确认任务真正结束才唤醒Driver重试finish_turn；迟到通知不能唤醒替换实例。不因错误已交付就假称推理退出，超时、根关闭和reply丢失保留各自分类。

结果经oneshot交回MCL，再由Driver显式history_append；累积器不直接写历史。finished仅表示显示流终态，不替代oneshot或真实退出证明。

## 预算

配置、请求体、SSE缓冲、响应、工具参数和发送队列均有上限。单消息图片最多8张、每张解码5MiB、合计20MiB；请求和响应总量各不超过32MiB，单SSE事件不超过4MiB。整体推理默认180秒，单连接发送等待最多100ms。预算错误不自动重试、续写或压缩。
