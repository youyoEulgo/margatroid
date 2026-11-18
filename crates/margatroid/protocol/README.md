# Protocol

Protocol定义纯JSON信封、请求、回执、分类状态、记录页与消息同步，只依赖标准库及serde等通用库，不依赖领域core、plugin、address或World。领域寻址、执行、请求关联和投递由DTO及各领域负责。

[DESIGN.md](DESIGN.md)定义45种wire消息及纯协议校验。本文与设计已采用Agent Address对外寻址；源码和运行产物的接入状态按实际实现与验证记录确认。

接口定义见项目上级目录的[WebSocket API](../../../../doc/api/WS_API.md)，接入链路见[API ENTRY](../../../../doc/api/API_ENTRY.md)。内部消息使用厂商无关结构、data媒体字段及真实sequence；历史通过规范SQL列重建完整消息。Agent设置文件使用setting.toml。

## 网络消息

WebSocket入口为`/ws`，业务使用JSON文本。入站信封为`{type,id,message}`，id只关联本连接中的在途请求；出站type之外的数据直接放在顶层，不再统一包入message。地址与定义路径都是纯网络值，不传内部Entity、World、进程句柄、数据库连接或执行能力。

目录共有45种wire type：18种client→server请求与27种server→client消息，全部在ClientMessage及ServerMessage中逐项对应。

```text
connection.register / connection.register.result
workspace.req / workspace.res / workspace.sync
workspace.start / workspace.start.result
workspace.stop / workspace.stop.result
workspace.restart / workspace.restart.result
workspace.reload / workspace.reload.result
agent.req / agent.res / agent.sync
agent.start / agent.start.result
agent.stop / agent.stop.result
agent.restart / agent.restart.result
agent.turn.req / agent.turn.res / agent.turn.sync
agent.turn.abort / agent.turn.abort.result
agent.task.req / agent.task.res / agent.task.sync
agent.history.req / agent.history.res
agent.log.req / agent.log.res / agent.log.sync
agent.mcl.req / agent.mcl.res / agent.mcl.sync
agent.message.chunk.sync / agent.message.full.sync
mcl.execute / mcl.execute.result
```

command必须回复原id的result，成功表示操作收口，不只受理。状态sync能够交付的内容不在result重复：Workspace/Agent生命周期、回合中止和Workspace重载成功只返回success=true；失败返回success=false及error。连接注册成功另返回实际client身份；MCL成功非Unit值另返回data，Unit省略data。状态没有变化也必须回复result，但不强行重复发送sync。result与sync没有固定先后顺序。

req/res与sync是无业务副作用的纯查询；command/result承载有副作用的操作。req只服务本次读取，投递在实际变化时从共享Client槽位筛选接收者，不依赖运行帧；实现已在当前工作树落地，wire字段不变。req/res显式读取数据；查询成功必须提供相应对象或集合，无数据时返回合法空数组。失败省略成功对象、集合及分页游标，不能用空数组伪装查询失败。sync不携带请求id，不结束操作或查询等待，不反过来充当回执。无法解析或识别的信封、未知type及缺失id的错误投递规则不在目录中私造新type，作为公共边界保留。

旧的state领域及BackendStateDto整体退出；不保留state.query/state.sync、workspace.definitions作用域、旧started/start_failed、history.result、mcl.command及独立delta/reasoning_delta/finished别名，不把它们包装成另一种全后端大包。

## 状态与消息值

workspace.req/res/sync反映全部已扫描Workspace，包括未激活项。每项统一包含path、definition、status、nullable address及自身error。definition中的name、project_root、agents是结构化文件定义，不重复平铺字段；定义成员只含name/driver/home_root，不附带成员状态或member_failures。当前扫描定义可能不同于运行实例启动时使用的定义，reload不会重启实例。

agent.req/res/sync只反映实际已激活实例，包含address、workspace、status、完整tools、sandboxes、sandbox、model及error，不为未激活定义成员生成占位项。后端通过Agent地址解析完整Entity并读取组件，VM生命周期标识只供内部使用。tools保留Tools的全部alias、path、description、input_schema、nullable owner_task_id及binding，静态和动态记录使用同一结构。

回合、任务、历史、会话日志、MCL记录与生成流均不塞入Agent状态对象。agent.turn提供指定实例的当前turn，空闲为null。agent.task查询与主动同步都是指定实例的完整活跃任务集合和各任务完整保留输出窗口；包含未return及已return未finish任务，不使用分块、输出追加补丁或removed列表。

内部AgentMessage是厂商无关结构，wire保留System、User、Assistant及Compact别名、ToolCall结果、Inject、Error及完整内容。reasoning和正文各一份，不出现replay、api_format、厂商签名块、加密重放内容或旧images别名。媒体数组使用data，当前媒体元素为media_type与标准base64 data。工具调用请求保存在Assistant.tool_calls，工具执行结果保存在type=tool_call，保留tool_call_id和tool_name。生成错误不是伪成功正文。

每个AgentMessage均有可选sequence。历史接口输出的sequence为本次真实持久记录序号；Block是否携带此字段由Driver决定，后端不据此反查历史、验证内容或推断关联。它不同于chunk中的分片sequence。AgentMessage与数据库消息行的关系为可逆拆装：SQL使用sequence/role/content/data/tool_call/payload，Assistant/Compact的SQL content承载reasoning和content对象，SQL payload只保存剩余字段；网络历史记录中的payload必须是完整重建的AgentMessage，不能直接把SQL剩余payload发出去。历史记录外层sequence、payload.sequence及full中的非null history_sequence指向同一条消息。

agent.history只读取agent_messages，不混入MCL或日志。agent.mcl读取mcl_records，保留原始source地址、turn、content及完整审计payload；其sequence与消息共用历史时间线，存在间隔是合法的。agent.log读取agent_logs，保留独立sequence、级别、可信source、stage、message及可选details/context/origin/raw_error/traceback；source=host仍仅指该Agent会话内的执行诊断，不表示推送daemon运行日志。

三类记录查询均使用排他before_sequence/after_sequence及正limit。双界要求after<before；无下界时取上界前最近一页，有下界时取区间内最早一页，返回总按sequence升序。成功响应带next_before_sequence和next_after_sequence，无对应下一页时为null；失败省略游标。日志与MCL的sync只发送新提交条目，不发全表，也不因其他状态变化重复发送旧记录；没有新增记录不发送空sync。历史与记录查询不消费数据或改变工具脚本游标。

agent.message.chunk.sync使用address/turn/generation关联一次生成，顶层content和reasoning为必填累计字符串；正文和思考共用该生成的快照sequence。agent.message.full.sync携带完整AgentMessage、nullable generation和history_sequence及error。它表达生成结束及完整内容，不恢复独立finished type。history_sequence为空不证明已落库；非空必须与message.sequence一致。消息同样可表达未经过chunk的用户输入、工具结果或错误，但不能把非生成消息冒充另一生成的完成。

mcl.execute.result的data按kind/value表达业务结果：messages、config、import、inference及block_snapshots。最后一项来自现有MCL的BlockSnapshots成功值，补齐上级早期草稿遗漏，不修改MCL语言。内部MCL Error映射success=false及error，不能继续用外层Ok包Error当网络成功。已执行但审计或交付失败时，必要提交事实可写error.details，不声称副作用从未发生，不自动重发。

## 校验

本轮同步投递已改为共享Client槽位直投，45种消息字段与编码保持。协议校验JSON、type、信封和字段形状、字节预算、角色内容及可选真实序号的合法范围；目标地址所属实例、调用身份、当前回合、命令受理及权限由DTO和领域验证。目标格式不得为了覆盖源码现状增加兼容字段。

现有独立预算仍为网络应用消息32MiB、id非空且最多512个UTF-8字节、网络错误message最多512个UTF-8字节；递归消息和媒体校验沿现有协议预算，媒体支持PNG/JPEG/GIF/WebP，每条最多8张、每张解码5MiB、合计20MiB。字段是字符限制还是字节限制必须按实际验证区分，不把JSON Schema字符数当字节预算。

ProtocolError是解析/编码错误类型；对外业务错误DTO采用code、message及可选details。错误不能泄漏凭据、图片、完整请求或原始Lua栈；Agent会话诊断日志的专门raw_error/traceback字段按该领域数据访问规则交付，不作为通用error.message。

公共未确认事项包括：分页默认值和最大值；状态候选值及过渡实例进入/移出时点；Workspace坏配置的表示；Agent重启参数取值；离线持久记录定位；零内容及落库失败full的精确交付边界；并发顺序、断线恢复及访问权限。这些边界没有通过新增订阅type、版本协商或通用错误type私自补齐，目录也不承诺事件重放。目标协议的设计状态与源代码实现状态不得混同。

## 生命周期载荷

workspace.start/stop/restart使用同形的{path}，目标来自已扫描定义；reload使用空对象并重扫后端配置来源，不接受任意扫描根目录。相应result只确认操作结果，Workspace自身状态由workspace.res/sync给出。停止不删除扫描定义，启动成功不等于全部成员激活。

agent.start使用{workspace,name}选择已激活Workspace的定义成员，不向客户端开放任意driver、home_root、数据库或setting.toml路径覆盖。agent.stop/restart使用{address}，后端解析目标Entity并校验当前状态；result只确认操作结果。

agent.turn.abort使用{address,turn}，后端定位目标Agent并匹配回合，命令结果只返回success/error。DTO沿既有控制入口受理，先清turn，再取消推理、强制return并终止未return工具，已return后台任务保留。控制收口不等于整个Agent停止；agent.turn.sync反映实际回合。

agent.task.req/res/sync读取状态不消费task_read游标、不清除replaced、不调用finish。finish真实清理后从集合移除，后续read/send/finish返回TaskMissing，无completed条目或完成缓存。

连接注册每个新WebSocket执行一次，成功返回实际Client地址及client_type，不把注册类别当认证。注册成功不隐含下载所有Workspace/Agent历史。断线不证明原命令未执行，具体在途收口与数据投递归公共连接及DTO职责，不在Protocol引入额外执行事件。

Workspace与Agent启动/重启的定义来源统一为“全量重载 → 发布缓存 → 从本次缓存快照启动”。workspace.reload只重载，不启动；没有针对单个Workspace独立回源的启动路径。任何全量加载失败都不使用旧缓存继续启动，重启先完整关闭旧实例，再由普通启动执行其校验与加载；不在关闭前预检新定义。网络字段不因此新增配置正文或缓存版本参数。

ServerMessage的Serialize与Deserialize共同校验成功/失败分支；成功分页两个游标始终输出、空值为null，失败省略游标和成功数据。ClientMessage::id返回&str，ServerMessage::id返回Option<&str>，sync始终为None。纯协议类型不会创建领域事件。

扫描来源已移除的运行Workspace继续展示其已知定义及真实状态，只允许停止。启动或重启必须在本次全量重载缓存中找到目标定义。sync每次发送从共享槽位按既定条件取得ClientSender，查询不改变接收范围。

外部字段统一使用Agent Address。MCL审计payload及Agent日志context_json只投影业务关联，VM生命周期标识在后端保留。网络投影在编码前移除内部VM标识。

Agent的sandboxes每次从该Entity的Sandboxes组件遍历，包含alias及path；sandbox来自SelectedSandbox。model由AgentInferenceConfig关联当前ModelRoutes，包含id、model、provider、api_type、context_window_tokens及可空thinking/reasoning_effort；配置或路由缺失为null。agent.req读取公共视图，sync由统一更新入口推进同一视图并投递，模型端点和凭据保留在后端。

Driver UI通过agent.ui.req/res/sync交付完整资源集合。查询以Agent Address定位实际Driver来源并沿原oneshot返回；同步独立监控ui目录变化。每个文件包含相对path、media_type和标准base64 data，内容revision使用SHA-256，完整集合和资源为空、读取失败均有明确分支。三种消息已实现，共享资料的Schema、AsyncAPI及实际后端校验同步45种消息。

## chunk.sync累计快照契约（已实现，尚未提交）

当前工作树已实现本节契约；旧49c7711产物仍为增量格式。新agent.message.chunk.sync保留address、turn、generation、sequence，删除delta，新增顶层必填字符串content、reasoning；两者是截至该sequence的完整已接受文本，空侧为""。至少一侧非空，不用null、缺省字段或旧名兼容表达累计文本。消息类型总数仍为45，full.sync字段和持久序号语义不变。

sequence是同generation文本快照顺序，前端直接替换而非拼接，跳号无需补片；同generation的重复或较小sequence忽略。full使该generation进入完成态，迟到chunk不能重开；full的正文、思考和元数据具有最终权威，失败时仍按原full.error/message表示，不把部分chunk当成功结果。

Rust类型、机器Schema和可执行样例已同步，AsyncAPI引用新Schema。前端及参考客户端源码未改；本轮只验证后端契约，不表示前端已对接新增能力。

生成full是一次性终态，不是落库状态的两阶段推送：推理完成发送一次history_sequence=null；保存同一带request_id的assistant/error后不再补发带持久序号的full。持久记录通过history查询关联。记录同步只负责非生成消息，generation=null，不能用普通MCL请求id推断生成关联。

显式可选turn契约：start_turn取得并返回当前turn，已有回合返回同一标识；推理和工具显式携带turn时严格校验，省略为None自由调用，不自动继承当前turn。自由调用仍受实际会话、权限、生命周期、取消及原有并发限制；Starting允许初始化所需自由调用，Error/Stopping拒绝。回合中止/收尾只作用于明确绑定该turn的请求，Agent关闭仍监督全部调用。详见本域DESIGN。运行中的旧进程仍需使用新构建。

历史compact消息保留type=compact及assistant同结构字段，推理边界按assistant处理。压缩推理原始生成full仍为assistant；Driver写入历史的compact记录通过普通持久记录同步交付，generation=null且携带真实history_sequence，不伪装成同一generation的第二条终态。前端需接受compact并按assistant布局或压缩摘要展示；本轮不改前端源码。

推理失败的mcl.execute.result为success=true、data.kind=inference，其value.message是error AgentMessage且value.stop_reason=error；原请求id保留。success仅表示MCL成功交付领域结果，不表示模型推理成功。compact()由调用方把成功compact或失败error写入历史，前端可查询并展示二者。

models.toml现支持每次推理前有界重读并按id选择，文件损坏/删除或id缺失均失败，不回退启动快照。既有ModelRoutes只服务最近安全公共投影；有效文件改动刷新Agent安全model信息，无效文件清空投影。
model.config.req/res/sync及model.config.update/update.result已实现，读取/同步原始TOML供编辑，修改完整校验后原子覆盖同一文件并保留权限；外部编辑/原子替换/删除也触发同一来源监控。无需Workspace、Agent或turn，不接收客户端文件路径，不新增持久目录/数据库或领域System/事件。接口遵循原注册、ingress、固定webui目标和id关联规则；前端源码未改。

Workspace/Agent重启是原停止和启动请求的顺序封装：停止成功后才启动，任一步失败原样回复；不因ToolTasks非空拒绝、不单独保留Entity、不提前reload或创建特殊重启状态。新实例重新创建Entity/Session，Address由普通启动规则产生，前端应按新目录同步处理旧来源退出和新来源出现。
