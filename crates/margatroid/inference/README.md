# Inference

本文描述当前源码中的推理契约，不表示运行中的daemon已经更新。具体入口与线程边界见本领域DESIGN。
inference_core定义推理请求、消息、结果、ActiveInference、模型配置能力及错误，只依赖mecs和通用库，不依赖client_core或其他业务core。公共模型变化通过ModelChanged与ModelObserverFactory注入通用回调；网络投递在plugin完成。inference_plugin组合Agent、Client、Tools记录，执行模型请求；不依赖其他plugin。MCL负责消息转换、批次工具调用和显式历史，Inference不维护上下文、不执行工具、不写历史。

InferenceRequest不携带tools参数。受理时遍历当前Agent的Tools，固定完整alias、description和input_schema快照；Tools为空时无工具定义。真实文件型模型配置在异步监督中重新读取并校验，再按本次路由构造HTTP；显式内存测试才可使用受理时已有路由。工具执行仍按当前Tools和登记Schema校验，推理不负责工具鉴权或执行。

Margatroid领域使用原生Axum装配及普通异步闭包。网络服务归本应用基础设施，mecs提供通用运行、异步事件和闭包能力。

## 配置与参数

InferencePlugin::open读取models.toml并完整校验模型路由，build安装ModelRoutes及HTTP客户端。provider与api_type分别明确指定，不从URL或模型名推断。请求携带路由选择键及采样参数；每次实际推理开始时通过ModelConfigFile重读来源，损坏、删除或ID缺失均失败，不回退旧路由。已构造的在途请求保持本次快照。ModelRoutes用于最近校验后的安全投影，不作为文件型执行的长期路由缓存。

| provider | api_type |
| --- | --- |
| openai | chat_completions、responses |
| anthropic | messages |
| deepseek | chat_completions、messages、responses |
| openrouter | chat_completions |

未知组合或不支持参数在HTTP前失败。base_url是API前缀，仅追加对应端点；Messages使用x-api-key及anthropic-version，其他格式使用Bearer。凭据、完整请求及敏感错误URL不进入日志或前端投影。非2xx响应有界读取错误正文，保留HTTP状态、上游message、type、code与request_id等诊断字段；剔除当前凭据，不转储完整响应。读取超时、断流、正文过大或无法解析时仍保留HTTP状态和读取情况，不将已知上游错误覆盖成笼统网络失败。

## 推理句柄初始化

新Agent创建System从inference_core同步挂载ActiveInference::default()，无需额外初始化往返。InitInference仍提供显式幂等初始化入口：已有组件不覆盖，未有组件才挂载current=None并通过原reply返回，不提交推理。

ActiveInference归inference_core，单独保存当前InferenceHandle。受理推理时写入本次请求、turn和控制句柄，已有未结束推理时拒绝重叠请求。中止直接取得当前句柄，先关闭StreamGate再取消，不查找或遍历ToolTasks。

## 输入与工具

InferenceRequest携带完整Agent Entity、显式可选turn、消息和唯一reply。Starting/Running允许turn=None的自由调用，不继承当前回合；Some(turn)必须非空、Agent为Running且与当前有效turn一致。失效关联返回空assistant的Completed，不启动HTTP或任务；Closing拒绝新执行。插件仅允许UPDATE或POST_UPDATE，中止控制先于推理受理。

推理消息由本领域独立定义，MCL在plugin层转换并展开Inject；core不引用消息、工具或Agent core。System保留原角色，中止回复作为普通完成消息处理，工具调用与结果必须正确配对。

工具来源只有当前Tools的全量记录，不经Driver、settings或visible_tools过滤，也不接收alias选择列表。准备本次请求时使用记录中的描述与schema，不读取工具文件；导入后文件metadata变化不会隐式更新记录。Tools为空时模型工具定义为空；不执行脚本。

## 请求适配

这里的代码结构是项目所有者明确规定的契约，不能以“效果相同”改成通用JSON修补器。

```rust
// 共享纯领域输入，不能承载厂商或API格式状态。
DomainRequest

// 插件内可失败的具体格式转换。
impl TryFrom<DomainRequest> for ChatCompletionsRequest
impl TryFrom<DomainRequest> for MessagesRequest
impl TryFrom<DomainRequest> for ResponsesRequest

// 格式枚举只用于分派，不是泛型类型参数。
enum ApiRequest {
    ChatCompletions(ChatCompletionsRequest),
    Messages(MessagesRequest),
    Responses(ResponsesRequest),
}

// 每个组合具有对应具体process实现。
OpenAi<ChatCompletionsRequest>
OpenAi<ResponsesRequest>
Anthropic<MessagesRequest>
DeepSeek<ChatCompletionsRequest>
DeepSeek<MessagesRequest>
DeepSeek<ResponsesRequest>
OpenRouter<ChatCompletionsRequest>
```

上述片段是类型与impl签名索引，字段和方法以设计及Rust定义为准。调用链固定为：

```text
第一层：厂商无关领域消息
    → DomainRequest / InferenceMessage
第二层：通用API格式转换
    → ApiAdapter::encode / TryFrom / try_into()
    → ChatCompletionsRequest、MessagesRequest或ResponsesRequest
第三层：厂商 × API格式特化
    → ApiAdapter::process_request
    → OpenAi<T> / Anthropic<T> / DeepSeek<T> / OpenRouter<T>::process
    → ProviderHttpRequest / HTTP

上游响应
    → 同一组合选择的FormatDecoder
    → 厂商无关ProviderFragment
    → 唯一ResponseAccumulator
    → InferenceResponse
    → MCL生成内部消息，由Driver显式记录
```

DomainRequest只携带消息、纯工具定义及公共采样语义；不含provider/api_type、URL、密钥、HTTP、Agent/turn执行身份、取消令牌或通道。第二层用具体格式的有类型消息保留assistant及reasoning-only身份；不再以EncodeInput把厂商能力传入通用转换，也不建立另一份持久消息。

禁止绕过这些边界：不能把三种具体Request合成一个任意JSON壳，不能用通用VendorAdapter按厂商补字段，不能将ApiRequest::Responses等枚举变体当作Rust泛型类型，不能从URL/模型名称猜格式，也不能用兼容handler兜底未知组合。JSON序列化是具体格式编码的输出，不是内部通用消息的替代品。

内部消息、共享响应、累积器、Block和历史不得加入replay、api_format、厂商thinking块、签名或encrypted_content。协议解码可以读取这些字段以识别上游事件，但不得将原生结构透传至领域消息，也不得为重放而再存一份reasoning。厂商选项与格式字段只在上述推理边界处理。

切换模型或API格式时，从同一份原始领域消息重新构造请求，不改写、清空数据库或替换角色。reasoning-only assistant是合法领域消息：允许没有正文和工具调用，原始reasoning保留在上下文及历史中。通用准入和第二层Responses转换不能统一拒绝或提前剔除它。禁止伪造签名、制造专有reasoning item或把reasoning包装成正文。

第三层的`OpenAi<ResponsesRequest>`将这类消息输出为`{"role":"assistant","content":""}`；这是最终请求投影补空正文，不要求修改领域消息。对不接受这种空assistant的Responses厂商，由其各自handler剔除本次请求中的该项。剔除不作用于领域上下文或历史，也不推广为所有Responses供应商的统一规则。有正文或工具调用的assistant必须保留这些内容及关联，不能因携带reasoning而整条删除。

源码已按此分层实现：ResponsesInput::ReasoningOnly在通用层保留，OpenAi与DeepSeek各自生成ResponsesHttpRequest最终投影。缺失及空字符串正文都覆盖；正文和工具调用非空的正常历史不被误删。运行中的daemon是否已使用此实现仍取决于部署版本。

这些不变量由具体格式转换、跨格式真实HTTP和存储往返测试验证：同一份内部消息可重新选择已支持组合；切换后正文与工具关联不变；内部reasoning只出现一次；返回值和落库消息不包含厂商重放字段。新增组合必须补充显式handler及对应编解码验证，不能只增加配置枚举。

第一层领域消息只表达业务内容；第二层TryFrom生成ChatCompletionsRequest、MessagesRequest或ResponsesRequest的通用格式材料；第三层OpenAi<T>、Anthropic<T>、DeepSeek<T>、OpenRouter<T>处理厂商差异后生成HTTP请求。ApiAdapter只分派明确支持的组合，T始终是具体格式请求类型。厂商选项、历史reasoning的映射、空assistant补齐或剔除均在第三层完成，不能把厂商限制混入领域准入或通用转换。参数处理使用有类型字段，不能先编码任意JSON再补丁修复。响应解码器由同一组合选定，逆向输出统一分片。Messages要求max_output_tokens，Responses不接受stop序列；无关的参数约束不因本次消息规则变化而放宽。

Responses提交完整本地上下文，store=false，不依赖远端conversation或previous_response_id。内部消息只保存一份reasoning、正文和结构化工具关联，不包含api_format、签名块、加密reasoning或replay。响应解码器将可读思考统一为reasoning，厂商协议签名及加密内容不进入共享结果、累积器、Block或历史。

历史正文、工具调用、工具结果及图片先映射通用API格式，再由具体厂商handler决定最终表达。支持独立reasoning输入的厂商映射该字段；不支持的厂商不发送该字段，但不能改写正文或伪造签名。纯reasoning消息按上文OpenAI补空正文、其他不支持的Responses厂商剔除请求项的规则处理，内部值始终保留。

对应回归应分别验证：领域准入接受reasoning-only；第二层不提前拒绝或丢失其身份；OpenAI Responses最终HTTP包含空正文assistant；不支持的Responses组合最终HTTP不含该项；有正文或工具的混合消息不被误删；切换厂商后仍从原始消息重新转换，领域消息和存储往返不变。插件单元测试和本地真实HTTP捕获已覆盖这些边界；测试不连接外部模型服务，也不修改用户历史。

内部用户和工具消息的图片集合JSON键统一为data；图片项保留media_type和base64 data。用户和工具结果图片均支持PNG/JPEG/GIF/WebP标准base64，校验数量、字节和magic。Messages与Responses使用原生工具图片；Chat保留tool结果，整批结果齐备后追加格式层user图片消息，不修改本地消息或历史。

## 单一流出口

HTTP/SSE读取任务只向一个有界ProviderFragment通道写入，不直接向前端推流。格式解码器维护SSE缓冲及协议状态，不另外维护最终正文。正文、思考、工具参数、usage和终态均通过该通道。

接收循环位于本次推理监督闭包，使用Tokio biased select!优先等待取消，每轮只读一个分片。追加前在StreamGate短锁内再次检查关闭状态；通过裁决才更新唯一ResponseAccumulator并分配sequence。只有已接受的正文或思考分片才能发送前端。

网络发送不持接受门锁，不因慢客户端阻塞接收循环。发送队列和缓冲有界；超过发送预算的连接停止接收本次流，不丢弃后端累积内容，也不把连接错误改成模型错误。发送能力持有共享Clients，每次接受片段后筛选当次webui连接，不固定起始sender快照；无前端仍能完成推理。中途连接可以收到后续sequence，但不重放已过去的分片，最终full及显式历史提供完整结果。

正常结束时验证对应格式的真实终态；通道关闭本身不表示协议成功。Chat检查finish_reason和DONE，Messages检查message_stop及内容块闭合，Responses检查其真实终态。工具调用校验完整ID、alias和JSON参数，不能执行半截调用。

## 中止与结果

DTO的WebSocket读取闭包把回合中止转为AbortTurnControl，由PRE_UPDATE同步控制处理；普通请求仍走DtoRequest。网络请求与领域执行通过原reply交接，不引入通用网络转发事件。

后端固定在PRE_UPDATE受理中止，先同步清turn，再调用ActiveInference.current取消句柄关闭本次StreamGate并触发取消；随后ToolTasks仅对未return任务原子强制回复并终止清理，已return后台任务保留。接受分片与关闭门共用同一短锁；门关闭后不再累积或准备发送任何新分片。receiver直接关闭并丢弃剩余队列，不继续排空，不等HTTP退出后才冻结。

前端点击中止立即拒绝本次request及实例的后续流，等待权威sync。后端受理存在网络及两帧延迟，因此sync仍可能校正少量内容；清理期间不再增加后端正文。

正常或用户中止的内容结果始终取自通道出口的ResponseAccumulator。上游任务退出后不能用另一份正文补写结果。中止冻结已接受的正文及思考，以completed普通结束，tool_calls为空，usage为空；无内容保持空值，不填充取消文案。

文本冻结与任务清理分开。已确认的provider或流错误立即经oneshot交给MCL转换为error消息，不等待后台join；正常取消在清理预算内按completed普通结束，超预算及时返回CleanupFailed。原监督任务始终保留并等待真实join，ActiveInference不提前释放，下一次重叠推理返回可见错误。清理完成通知绑定完整Agent Entity、Agent.instance、request_id和本次生成租约。若任务实际join尚未完成，独立异步等待wait_finished后提交一次World闭包，不逐帧重发通知；复核同一实例和句柄后唤醒Agent.wake_slot，迟到通知不作用于替换来源。不因错误已交付就假称推理退出，超时、根关闭和reply丢失保留各自分类。

结果经oneshot交回MCL，再由Driver显式history_append；累积器不直接写历史。生成分片使用agent.message.chunk.sync，字段为address、turn、generation、sequence、content和reasoning；后两者均为截至本sequence的完整累计文本。

生成终态使用agent.message.full.sync，带完整内部消息、同一generation、history_sequence=null及nullable error。成功消息包含完整正文、思考、工具调用、用量和停止原因；tool_calls使用tool_name及JSON文本arguments，与MCL内部AgentMessage一致。失败消息为type=error、message及request_id，外层error使用code/message。Inference不确认落库，因此message不带持久sequence、不填history_sequence，不另发旧finished消息。

full不替代oneshot或真实退出证明。流/协议错误及时交付后仍等待worker真实清理；清理超预算若原reply已交付，仅记录诊断并继续真实监督，不向同generation补发第二条终态，不重新回复。中止成功的full只保留接受门关闭前冻结的正文与思考，不能混入晚到分片。

## 预算

推理不做本地请求体字节检查，也不接受max_request_bytes路由配置。HTTP413以及明确的payload_too_large_error、payload_too_large、request_too_large、request_body_too_large机器code/type归一为RequestBodyTooLarge，保留原HTTP诊断文本及可取得的关联。details包含http_status、可选request_id/provider_type/provider_code，不编造实际大小或供应商上限；Driver根据稳定code执行恢复。普通invalid_request_error、token上下文超限、550空正文、502和连接重置保留其原分类，不能猜作请求体超限。隐藏的128MiB请求、4MiB单条文本和16MiB全文本拦截已删除。配置、SSE缓冲、响应及输出工具参数的独立预算保留：单消息图片最多8张、每张解码5MiB、合计20MiB，响应总量128MiB，单SSE事件4MiB。整体推理默认180秒。

推理通过普通异步闭包进行短启动交接，监督任务由共享InferenceHandle持有；未交接的Pending句柄仍算活动，wait_finished等待真实join，支持多个等待者和等待者取消。HTTP客户端不缓存Tokio执行器Handle、不使用临时服务握手；前端出口使用client_core::ClientSender和ClientFrame，不依赖mecs服务器类型。

工具目录不按条目数或单条描述长度截断，统一受HTTP请求总字节预算约束。真实HTTP回归覆盖1024个工具、每条16KiB描述，在三种模型格式中逐条核对完整名称、描述及schema。

推理受理阶段固定Tools定义、输入及实际Agent身份，文件型配置在监督中重读后编码HTTP。闭包持有本次owned定义、网络、流与清理材料，不跨await借World。未知工具名只要响应结构合法就保留，不在推理结果层按工具清单拒绝；当前Tools存在性和schema校验由工具调用受理负责。内部取消信号和错误类别保留，正常中止向调用方返回completed，不影响真实清理要求。

## chunk.sync累计文本快照

当前源码的累计文本出口不提供旧delta别名；实际运行服务和客户端版本须分别确认。

上游ProviderFragment先经接受门写入唯一ResponseAccumulator，实际正文/思考变化后，在同一临界区取得sequence、完整content及完整reasoning的owned快照，锁外发送。前端不再维护字符串累积器，每条chunk替换同generation的正文和思考；生成结束的full.sync保持原结构，负责最终校正、工具调用、usage、结束原因及错误。工具参数和usage仍正常进入后端累积器，但不单独触发文本快照。

新wire删除delta，使用顶层必填content和reasoning字符串，空值为""。这是不兼容改版，不接受旧增量字段作为别名。sequence只在正文或思考实际变化时递增，中途加入可从第一条收到的累计快照直接显示当前文本，不承诺无后续变化时自动补发旧快照或断线重放。

累计快照包含全部已接受文本，只改变前端出口，不改变厂商解码、消息存储和后端唯一累积器。每次克隆/编码的成本随累计文本增长，不再设置32MiB网络帧上限，慢连接的有界队列及背压规则保持；不因单帧字节数取消客户端，也不静默截断或改回增量。取消门关闭后不再接受新片段，full仍从同一冻结累积器生成。

验证要求：思考与正文交错时每次含两侧累计值；拆分UTF-8不产生损坏字符；工具参数/usage不误发文本快照；丢失某个chunk后的更高sequence可直接恢复文本；重复/迟到chunk不回滚，full后chunk不能重开生成；正常、取消、错误及零文本结束沿原full；预算失败不影响后端最终结果。

同一generation的full只由推理消费链路交付一次；后续历史持久化不重发带request_id的assistant/error，持久序号通过history查询取得。已交付生成错误后的清理超时只记诊断，继续保留真实监督，不发送第二条终态full。

显式可选turn契约：start_turn取得并返回当前turn，已有回合返回同一标识；推理和工具显式携带turn时严格校验，省略为None自由调用，不自动继承当前turn。自由调用仍受实际会话、权限、生命周期、取消及原有并发限制；Starting允许初始化所需自由调用，Closing拒绝。回合中止/收尾只作用于明确绑定该turn的请求，Agent关闭仍监督全部调用。详见本域DESIGN。运行中的旧进程仍需使用新构建。

MCL边界将推理领域Err转换为Inference结果内的error AgentMessage（stop_reason=error、原request_id），与顶层MCL执行错误区分；推理领域内部仍使用原Result/清理监督，不在失败时假造assistant或自动重试。

models.toml现支持每次推理前有界重读并按id选择，文件损坏/删除或id缺失均失败，不回退启动快照。既有ModelRoutes只服务最近安全公共投影；有效文件改动刷新Agent安全model信息，无效文件清空投影。
model.config.req/res/sync及model.config.update/update.result已实现，读取/同步原始TOML供编辑，修改完整校验后原子覆盖同一文件并保留权限；外部编辑/原子替换/删除也触发同一来源监控。无需Workspace、Agent或turn，不接收客户端文件路径，不新增持久目录/数据库或领域System/事件。接口遵循原注册、ingress、固定webui目标和id关联规则；前端源码未改。

InferenceHandle持有本次生成的source_alive租约，来源有效性不再从Agent.source_lease取得。Agent实例只有Starting/Running/Closing；内部来源实例不作为网络目标字段。实体地址为绝对节点路径，文件资源type:scope/name:tag定位规则不变。

普通InferenceHandle.cancel只冻结接受门并请求取消，保留共享来源租约，使同generation的终态full能够交付。retire_source单独用于Agent真正关闭或来源替换，淘汰旧实例的已入队帧；调用方随后仍需cancel并等待真实清理。两者不可混同，回合中止不能提前退休终态来源。共享lease取消回归验证冻结文本、一次full、真实join及迟到文本拒绝。


上游明确请求体超限时，error full与MCL推理结果都携带code=RequestBodyTooLarge及安全details；无响应体的413仍能归类。HTTP非成功错误仍只读取有界诊断体并剔除当前认证内容，不把原始请求或凭据加入详情。


执行级tag由InferenceRequest.tag受理时固定到StreamDelivery；Assistant Driver普通推理显式Some("inference")，压缩推理Some("compact")，均在chunk/full顶层输出相同字符串；其他调用方省略tag时仍输出必填null。成功、错误和取消冻结后的终态沿同一generation及来源租约保留tag；退休来源的旧帧仍按原规则丢弃。tag不进入DomainRequest、厂商HTTP、InferenceMessage、assistant/error内容或历史，不改变turn和取消范围。非空标记须非全白、无控制字符且最多512字节，否则InvalidParameters。
