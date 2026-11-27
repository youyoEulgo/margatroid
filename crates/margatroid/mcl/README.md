# MCL

mcl_core定义语法树、队列、请求和结果，只依赖mecs及通用库。消息在MCL契约中使用结构化值，执行时由mcl_plugin转换并校验所属领域消息类型；core不引用其他领域core。mcl_plugin依赖address和所需领域core，不依赖其他plugin。

本文同步本轮实现契约，不表示运行中的daemon已更新。IMPORT TOOL只向Agent的唯一Tools集合增加alias、description、input_schema和path记录，不维护选择名单。inference与tool_call使用当前Tools，消息作为一个输入，另可显式传入可选turn；tool_call支持调用数组及消息tool_calls投影。tools:register、原始进程通信和MCP协议由通用工具宿主及工具侧处理，MCL使用现有指令执行领域调用。

## Block快照指令

```text
EFFECT block_snapshot ?
arguments: ["session"]

EFFECT block_snapshots ?
arguments: ["session"]

EFFECT block_restore ? ?
arguments: ["session", 1]
```

block_snapshot和block_restore成功返回Unit；block_snapshots返回BlockSnapshots，包含指定Block的timeline与快照映射。参数复用现有问号参数；指令沿MCL FIFO等待存储调用完成，通过既有回复及审计出口收口。文件、映射、复制及原子恢复规则由[Storage设计](../storage/DESIGN.md)定义。

## 指令

```text
IMPORT MESSAGE prompt:margatroid/assistant-soul:dev AS soul
IMPORT TOOL tool:margatroid/bash:dev AS bash
IMPORT SANDBOX sandbox:margatroid/workspace-write:dev AS workspace_write
CREATE TEMP BLOCK msg (instructions, compact_prompt, compact_context)
CREATE TEMP BLOCK entry (pending)
CREATE BLOCK session (conversation)
GET session.conversation
GET entry.pending[0]
INJECT soul TO msg.instructions
INJECT ? TO session.conversation[-1]
INJECT [] TO entry.pending[0,1]
CONFIG GET
CONFIG SET ? ?
CONFIG DELETE ?
EFFECT inference (msg.instructions, session.conversation)
EFFECT tool_call session.conversation[-1].tool_calls
EFFECT sandbox_use ?
EFFECT history_append ?
EFFECT start_turn
EFFECT finish_turn
EFFECT ready
EFFECT wake
```

Lua调用为margatroid.mcl:execute(target_address, text, arguments)，arguments按顺序对应问号。目标使用完整节点地址，来源及实际执行instance由宿主绑定，不接受脚本伪造source。宿主只依赖mcl_core发送MclCommand并等待reply；唯一执行入口MclCommand要求调用方显式提供source_address，网络入口及Lua宿主传入自身真实地址，与source实体身份核对，MCL审计source保存该地址而非Entity编号；history_append按消息type映射role及规范内容列，不保存调用来源，返回带真实sequence的完整消息；mcl_plugin不定义Lua函数。inference使用消息数组；tool_call使用结构化调用数组或消息的tool_calls投影。marker形式为EFFECT inference ? [ ? [ ? ]]和EFFECT tool_call ? [ ? ]，第二参数为显式turn，不是工具alias列表。inference第三参数可显式提供tag，两个附加位置均须问号；仅这种三参数形式允许第二参数null表示无turn。tag必须非空、非全白、无控制字符且最多512字节，只用于本次chunk/full顶层，不进入厂商请求或持久消息。配置路径和值分别占一个参数。

## 消息Block

Block字段只保存消息数组，创建时不写类型。普通Block存于固定timeline SQLite文件，TEMP及其快照只在内存中按Agent.instance隔离；持久子项各占一张表，消息按sequence/role/content/data/tool_call/payload逐行保存。CREATE不存在则创建，存在且字段一致则打开，不清空内容；不返回created标志。

GET返回.Array.Message，单索引也返回数组。索引从0开始，-1为末项；非负范围左闭右开，负范围左开右闭，端点同号。多来源按书写顺序拼接，Prompt alias每次按路径读取。工具和沙箱alias不作为消息GET来源。

INJECT无目标索引时整体替换；单索引插入，非负插到元素前、负索引插到元素后；范围执行替换。空数组清空字段或删除选定范围。存储层固定来源视图后读取全部来源，再在目标Block文件单事务中校验及修改；跨Block不承诺单个SQL事务，失败保留目标原值。已提交操作不因稍后取消而伪称回滚。

## 设置

CONFIG GET返回.Config，读取当前Agent完整setting.toml。CONFIG SET ? ?传入点分路径及TOML值，没有键则新增，存在则整体替换，不追加或合并。CONFIG DELETE ?传入点分路径，删除指定键，不自动删除空父table。

SET创建缺失的中间table；中间节点是非table时返回错误。DELETE目标不存在视为无修改成功。键路径不支持数组索引，不用nil或空数组表示删除；TOML不支持的值报错。日期时间等TOML值由宿主桥接明确转换，不把普通字符串猜成日期。

配置只访问当前Agent设置文件，不接收任意文件路径。修改串行执行并以完整文件替换持久化；失败保留原文件，不在Agent上缓存第二份设置正文。模板只在Agent设置不存在时初始化，重启不覆盖已有设置。

## 导入与效果

IMPORT校验种类、依赖和alias后，在对应资源领域登记；TOOL保存路径、描述及schema的完整记录，只增加工具，不另设可见名单。alias在同Agent内跨类别唯一；冲突不覆盖旧登记。工具描述及schema在导入时缓存于Tools，文件metadata变化不会隐式更新记录；执行按记录path读取程序，按记录schema校验参数。MCL不保存通用ImportBindings、ResourceID或工具/沙箱正文仓库。

| EFFECT | 参数 | 返回值 |
| --- | --- | --- |
| inference | 消息数组、可选显式turn、可选显式tag | .Inference.message、.Inference.stop_reason |
| tool_call | 调用数组或消息tool_calls投影、可选turn | .Array.Message |
| sandbox_use | 零或一个沙箱alias组成的数组 | Unit |
| history_append | 消息数组 | .Array.Message（带真实sequence） |
| block_snapshot | Block逻辑名字符串 | Unit |
| block_snapshots | Block逻辑名字符串 | .BlockSnapshots |
| block_restore | Block逻辑名字符串、正整数快照ID | Unit |
| start_turn | 无 | .Turn（当前真实回合ID） |
| finish_turn | 无 | Unit |
| ready | 无 | Unit |
| wake | 无 | Unit |

inference和tool_call将请求交给所属领域并等待oneshot。tool_call先发出整批带独立oneshot的请求，再通过join_all收齐并保持输入顺序；保留原调用ID，每个工具调用错误均转换为failed工具消息，包括配置、参数、执行、超时、清理、任务panic、回复丢失和关联错误；不因单个失败丢失整批结果。无法解析出有效工具调用的命令错误与其他非工具错误，通过唯一回复出口转换为Error值，形状为{"Error":{"type":"error","message":"错误种类: 说明","request_id":"本次命令id"}}，不抛Lua宿主异常终止Driver。原始执行失败仍在MCL审计中记录succeeded=false。清理尚未证明时及时返回错误并保留后台监督，不将任务伪报completed或安全退出。sandbox_use选择alias，不保存长期策略正文；实际执行读取选定路径的策略。history_append显式保存消息；其他操作不自动追加消息历史。指令审计与消息历史分别记录，不把审计参数投影成重复消息。两类历史以Agent内sequence标识并排序，不设历史key；消息按规范列拆装，payload仅保存剩余字段；历史追加每次产生新记录并返回新sequence；Block可选保留此字段，不做历史关联校验。关联、去重和查找策略由Driver自行决定，当前不提供额外关联查询。

ready/start_turn/finish_turn调用agent_core同步扩展，wake操作目标Agent自有的wake_slot，不依赖Driver VM或旧AgentWaitSlot。wait由公共LuaServices等待目标AgentWakeSlot，不是MCL指令。model_set首次创建配置时从ModelObserverFactory取得通用回调。

sandbox_use首次使用时从SandboxObserverFactory创建缺失的空Sandboxes/SelectedSandbox。空alias数组可初始化为未选择；非空alias必须已经真实导入，否则失败，不自动创建策略记录。随后由sandbox_core统一选择入口提交变化。上述跨域装配只在plugin完成。

## 队列与取消

每个目标仍按FIFO分派。CONFIG、Block、IMPORT及普通状态effect先持Arc<AtomicBool>执行锁，真实执行及审计完成后解锁并直接oneshot回复；工具、推理及finish_turn异步不锁队列，工具内部可向同Agent调用MCL。active仅用于在途身份/重复id及关停核对，不承担结果队列。删除CommandOutput/CommandError完成事件及pending；完成guard负责移除active、更新drain和一次实际解锁唤醒，不跨await借World，不轮询等结果。

插件仅允许UPDATE或POST_UPDATE，回合中止固定PRE_UPDATE同步控制，不进入FIFO，不关闭整个队列。中止先清目标turn，只取消绑定该turn的推理及未return工具；自由调用、其他回合和已return后台任务保留。finish_turn在World外等待本轮推理真实收口，再核对关联并清turn，不轮询重试，不移除其他请求的句柄。Agent关闭仍清理所有调用。

关闭队列拒绝新命令、取消排队和在途操作，仍等待文件worker、领域任务和回执真实收口。drain成功不只表示发出了取消；清理失败保持错误，不以丢弃Future代替退出证明。


Workspace启动Agent事件只发起Driver工具调用。创建注入方法检查节点及Driver归属、准备或复用文件、打开持久化连接并登记，发送StartAgent<AgentStorage>；System同步挂载实体及组件，绑定真实parent/task_id后回复。Agent状态为Starting/Running/Closing；Workspace状态为Running/Deleting/Error，二者不共用旧四态。

Agent关闭先按节点找到Agent，再按其parent和task_id关闭父实体任务集合内的创建任务；任务直接等待绑定AgentHandle的关闭，不重新发送StopAgent形成回路。Return仅交付结果，不结束任务生命周期。创建、取消和关闭失败均保留真实监督，不以丢弃Future当完成。

MCL状态门统一只允许Starting/Running，不根据effect判断回合。推理和工具调用只携带显式可选turn，省略为None，不抓取当前上下文自动绑定；Some时的Running/有效turn及匹配由Inference/Tool实际受理入口负责。MCL不伪造无turn的推理完成或工具中止结果，也不预先判断工具alias存在性。ready/start_turn/finish_turn的约束由Agent core统一方法承担。

Agent创建注入接口为node_name/alias/project_root/config_path/ui_path，拒绝旧resource_dir创建参数。配置仅首次从config_path初始化至Agent持久setting.toml，后续重启保留已修改配置；ui_path始终直接引用与监控，不创建Agent本地UI，不复制、覆盖、删除已有UI副本，也不把旧副本作为回退来源。Lua程序加载只读取程序文件，不推导或读取同目录配置。

显式可选turn契约：start_turn取得并返回当前turn，已有回合返回同一标识；推理和工具显式携带turn时严格校验，省略为None自由调用，不自动继承当前turn。自由调用仍受实际会话、权限、生命周期、取消及原有并发限制；Starting允许初始化所需自由调用，Closing拒绝。回合中止/收尾只作用于明确绑定该turn的请求，Agent关闭仍监督全部调用。详见本域DESIGN。运行中的旧进程仍需使用新构建。

推理领域的连接失败、HTTP错误、超时等通过Inference.message返回type=error的AgentMessage，附原request_id及stop_reason=error；不再提升为顶层MCL Error导致Driver退出。执行通道错误、MCL受理/参数/权限错误及真正的清理失败仍按各自原契约收口。压缩消息的历史写入、快照及上下文替换由compact调用方处理。

GET和后续SET是两条独立命令，Lua编辑数组后整份SET覆盖最新键；中间其他SET的结果可能被覆盖，按当前契约接受，不提供隔离、token、CAS或upsert。配置单次实际提交仍原子，CONFIG/Block的单条顺序由执行锁保证。

实体寻址使用/demo/coder等绝对节点路径；IMPORT中的文件资源继续使用type:scope/name:tag且tag显式非空。节点路径不替代资源定位规则。
