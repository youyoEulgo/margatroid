# Protocol

Protocol定义纯JSON信封、请求、回执、分类状态、记录页与消息同步，只依赖标准库及serde等通用库，不依赖领域core、plugin、address或World。领域寻址、执行、请求关联和投递由DTO及各领域负责。

[DESIGN.md](DESIGN.md)定义42种wire消息及纯协议校验。本文与设计已采用Agent Address对外寻址；源码和运行产物的接入状态按实际实现与验证记录确认。

接口定义见项目上级目录的[WebSocket API](../../../../WS_API.md)，接入链路见[API ENTRY](../../../../API_ENTRY.md)。内部消息使用厂商无关结构、data媒体字段及真实sequence；历史通过规范SQL列重建完整消息。Agent设置文件使用setting.toml。

## 网络消息

WebSocket入口为`/ws`，业务使用JSON文本。入站信封为`{type,id,message}`，id只关联本连接中的在途请求；出站type之外的数据直接放在顶层，不再统一包入message。地址与定义路径都是纯网络值，不传内部Entity、World、进程句柄、数据库连接或执行能力。

目录共有42种wire type：17种client→server请求与25种server→client消息，全部在ClientMessage及ServerMessage中逐项对应。

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

req/res与sync是无业务副作用的纯查询；command/result承载有副作用的操作。req只服务本次读取，sync每帧筛选符合条件的ClientSender后发送。req/res显式读取数据；查询成功必须提供相应对象或集合，无数据时返回合法空数组。失败省略成功对象、集合及分页游标，不能用空数组伪装查询失败。sync不携带请求id，不结束操作或查询等待，不反过来充当回执。无法解析或识别的信封、未知type及缺失id的错误投递规则不在目录中私造新type，作为公共边界保留。

旧的state领域及BackendStateDto整体退出；不保留state.query/state.sync、workspace.definitions作用域、旧started/start_failed、history.result、mcl.command及独立delta/reasoning_delta/finished别名，不把它们包装成另一种全后端大包。

## 状态与消息值

workspace.req/res/sync反映全部已扫描Workspace，包括未激活项。每项统一包含path、definition、status、nullable address及自身error。definition中的name、project_root、agents是结构化文件定义，不重复平铺字段；定义成员只含name/driver/home_root，不附带成员状态或member_failures。当前扫描定义可能不同于运行实例启动时使用的定义，reload不会重启实例。

agent.req/res/sync只反映实际已激活实例，包含address、workspace、status、完整tools、sandboxes、sandbox、model及error，不为未激活定义成员生成占位项。后端通过Agent地址解析完整Entity并读取组件，VM生命周期标识只供内部使用。tools保留Tools的全部alias、path、description、input_schema、nullable owner_task_id及binding，静态和动态记录使用同一结构。

回合、任务、历史、会话日志、MCL记录与生成流均不塞入Agent状态对象。agent.turn提供指定实例的当前turn，空闲为null。agent.task查询与主动同步都是指定实例的完整活跃任务集合和各任务完整保留输出窗口；包含未return及已return未finish任务，不使用分块、输出追加补丁或removed列表。

内部AgentMessage是厂商无关结构，wire保留System、User、Assistant、ToolCall结果、Inject、Error及完整内容。reasoning和正文各一份，不出现replay、api_format、厂商签名块、加密重放内容或旧images别名。媒体数组使用data，当前媒体元素为media_type与标准base64 data。工具调用请求保存在Assistant.tool_calls，工具执行结果保存在type=tool_call，保留tool_call_id和tool_name。生成错误不是伪成功正文。

每个AgentMessage均有可选sequence。未落历史时省略，已落历史时为真实持久消息序号，不能按数组位置或Block局部编号生成。它不同于chunk中的分片sequence。AgentMessage与数据库消息行的关系为可逆拆装：SQL使用sequence/role/content/data/tool_call/payload，Assistant的SQL content承载reasoning和content对象，SQL payload只保存剩余字段；网络历史记录中的payload必须是完整重建的AgentMessage，不能直接把SQL剩余payload发出去。历史记录外层sequence、payload.sequence及full中的非null history_sequence指向同一条消息。

agent.history只读取agent_messages，不混入MCL或日志。agent.mcl读取mcl_records，保留原始source地址、turn、content及完整审计payload；其sequence与消息共用历史时间线，存在间隔是合法的。agent.log读取agent_logs，保留独立sequence、级别、可信source、stage、message及可选details/context/origin/raw_error/traceback；source=host仍仅指该Agent会话内的执行诊断，不表示推送daemon运行日志。

三类记录查询均使用排他before_sequence/after_sequence及正limit。双界要求after<before；无下界时取上界前最近一页，有下界时取区间内最早一页，返回总按sequence升序。成功响应带next_before_sequence和next_after_sequence，无对应下一页时为null；失败省略游标。日志与MCL的sync只发送新提交条目，不发全表，也不因其他状态变化重复发送旧记录；没有新增记录不发送空sync。历史与记录查询不消费数据或改变工具脚本游标。

agent.message.chunk.sync使用address/turn/generation关联一次生成，delta只含新增content或reasoning；正文和思考共用该生成的分片sequence。agent.message.full.sync携带完整AgentMessage、nullable generation和history_sequence及error。它表达生成结束及完整内容，不恢复独立finished type。history_sequence为空不证明已落库；非空必须与message.sequence一致。消息同样可表达未经过chunk的用户输入、工具结果或错误，但不能把非生成消息冒充另一生成的完成。

mcl.execute.result的data按kind/value表达业务结果：messages、config、import、inference及block_snapshots。最后一项来自现有MCL的BlockSnapshots成功值，补齐上级早期草稿遗漏，不修改MCL语言。内部MCL Error映射success=false及error，不能继续用外层Ok包Error当网络成功。已执行但审计或交付失败时，必要提交事实可写error.details，不声称副作用从未发生，不自动重发。

## 校验

协议校验JSON、type、信封和字段形状、字节预算、角色内容及可选真实序号的合法范围；目标地址所属实例、调用身份、当前回合、命令受理及权限由DTO和领域验证。目标格式不得为了覆盖源码现状增加兼容字段。

现有独立预算仍为网络应用消息32MiB、id非空且最多512个UTF-8字节、网络错误message最多512个UTF-8字节；递归消息和媒体校验沿现有协议预算，媒体支持PNG/JPEG/GIF/WebP，每条最多8张、每张解码5MiB、合计20MiB。字段是字符限制还是字节限制必须按实际验证区分，不把JSON Schema字符数当字节预算。

ProtocolError是解析/编码错误类型；对外业务错误DTO采用code、message及可选details。错误不能泄漏凭据、图片、完整请求或原始Lua栈；Agent会话诊断日志的专门raw_error/traceback字段按该领域数据访问规则交付，不作为通用error.message。

公共未确认事项包括：分页默认值和最大值；状态候选值及过渡实例进入/移出时点；Workspace坏配置的表示；Agent重启参数取值；离线持久记录定位；零内容及落库失败full的精确交付边界；并发顺序、断线恢复及访问权限。这些边界没有通过新增订阅type、版本协商或通用错误type私自补齐，目录也不承诺事件重放。目标协议的设计状态与源代码实现状态不得混同。

## 生命周期载荷

workspace.start/stop/restart使用同形的{path}，目标来自已扫描定义；reload使用空对象并重扫后端配置来源，不接受任意扫描根目录。相应result只确认操作结果，Workspace自身状态由workspace.res/sync给出。停止不删除扫描定义，启动成功不等于全部成员激活。

agent.start使用{workspace,name}选择已激活Workspace的定义成员，不向客户端开放任意driver、home_root、数据库或setting.toml路径覆盖。agent.stop/restart使用{address}，后端解析目标Entity并校验当前状态；result只确认操作结果。

agent.turn.abort使用{address,turn}，后端定位目标Agent并匹配回合，命令结果只返回success/error。DTO沿既有控制入口受理，先清turn，再取消推理、强制return并终止未return工具，已return后台任务保留。控制收口不等于整个Agent停止；agent.turn.sync反映实际回合。

agent.task.req/res/sync读取状态不消费task_read游标、不清除replaced、不调用finish。finish真实清理后从集合移除，后续read/send/finish返回TaskMissing，无completed条目或完成缓存。

连接注册每个新WebSocket执行一次，成功返回实际Client地址及client_type，不把注册类别当认证。注册成功不隐含下载所有Workspace/Agent历史。断线不证明原命令未执行，具体在途收口与数据投递归公共连接及DTO职责，不在Protocol引入额外执行事件。

Workspace与Agent启动/重启的定义来源统一为“全量重载 → 发布缓存 → 从本次缓存快照启动”。workspace.reload只重载，不启动；没有针对单个Workspace独立回源的启动路径。任何全量加载失败都不使用旧缓存继续启动，重启在新缓存目标校验成功后才关闭原实例。网络字段不因此新增配置正文或缓存版本参数。

ServerMessage的Serialize与Deserialize共同校验成功/失败分支；成功分页两个游标始终输出、空值为null，失败省略游标和成功数据。ClientMessage::id返回&str，ServerMessage::id返回Option<&str>，sync始终为None。纯协议类型不会创建领域事件。

扫描来源已移除的运行Workspace继续展示其已知定义及真实状态，只允许停止。启动或重启必须在本次全量重载缓存中找到目标定义。sync的ClientSender每帧依据既定条件取得，查询不改变接收范围。

外部字段统一使用Agent Address。MCL审计payload及Agent日志context_json只投影业务关联，VM生命周期标识在后端保留。网络投影在编码前移除内部VM标识。

Agent的sandboxes每次从该Entity的Sandboxes组件遍历，包含alias及path；sandbox来自SelectedSandbox。model由AgentInferenceConfig关联当前ModelRoutes，包含id、model、provider、api_type、context_window_tokens及可空thinking/reasoning_effort；配置或路由缺失为null。agent.req和每帧sync使用同一实时投影，模型端点和凭据保留在后端。
