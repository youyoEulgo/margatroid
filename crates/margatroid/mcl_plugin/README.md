# MCL Plugin

显式message历史策略已实现：推理和工具调用只返回结果，所有message历史仅由显式MCL history_append控制。MCL指令审计仍自动记录，审计payload中的消息不作为独立message记录。示例Driver对需要保留的结果另发显式追加指令。


当前实现说明见 [DESIGN.md](<DESIGN.md>)。旧 REF/MERGE/BIND/LOAD/EXPOSE、visibility 与 history_record 路径已删除，不再受理。

## 轮次中止

MCL已实现Idle/Working轮次effect、准备完成后的当前turn门、正常形状取消结果及finish清理校验。控制事件由AgentPlugin的PRE_UPDATE系统处理，不进入FIFO；自定义调度需确保控制先于本插件。完整跨插件规则及Driver分支见[设计](<DESIGN.md>)。客户端手工MESSAGE参数中的递归User INJECT在Working时拒绝；agent来源普通写入仍允许。别名/selector读取内容不作为用户消息入口，外部用户发送入口须使用受检手工参数。

- Agent运行期由`Running`改为`Idle / Working`，保留Creating/Stopping。启动Idle；`start_turn`仅Idle生成turn并置Working，不覆盖；`finish_turn`在本轮真实清理完成后清turn并转Idle。重复Idle完成幂等，旧关联不能覆盖新Working。
- abort为独立控制事件，先于MCL继续及推理/工具受理，不入FIFO。Working立即清turn但保持Working至Driver收尾；Working且turn=None重复abort幂等pending。协议目标`agent.turn.abort`要求非空turn_id，关联`agent.turn.abort_result`的Ok仅证明受理，不证明清理完成。前端及服务端用户消息受理均只允许Idle，Working只允许abort。
- `inference/tool_call`必须在异步审计/参数准备之后的实际effect动作处检查turn；无turn不提交执行，返回Ok正常形状取消结果。不新增`get_turn`，Driver不检查turn。
- 五种情况（abort前Working且turn存在）为：inference未受理/已受理、tool_call未受理/已受理、Driver间隙。未受理outstanding请求claim一次性reply一次且禁止迟到执行；已受理取消token并等待真实清理；Driver间隙由下一effect的实际动作turn门覆盖。
- Assistant消息自身携带`stop_reason=Cancelled`，序列化为`cancelled`，保留已有正文和思考，无输出保持空值、空tool_calls、usage=None；保持正常Inference外层形状。工具取消结果保持原调用ID、顺序及身份关联，`failed=true`，正文`Tool call aborted.`。stop_reason贯穿消息、Block、历史及投影。
- Driver读取第一条entry并移除同一第一条，显式历史保留原值，各分支自行更新上下文。Inject只递归展开容器并追加非容器上下文，跳过包括嵌套的取消Assistant，不执行子消息控制分支。User追加上下文/start_turn/推理；取消Assistant仅历史，正常Assistant入上下文，无tool_calls是唯一finish_turn路径，否则发工具批次；全部工具结果入上下文，批次完成即使turn=None仍推理，取得取消Assistant完成收尾。
- Error不得直接finish_turn；真实失败不得伪造cancelled Assistant。清理失败保持Err、Working及未清理活动，不虚假转Idle。轮次abort不改变Creating/Stopping、StopAgent根取消、MCL回执drained及真实清理的原语义。

## 已实现的数据库 Block

运行时所有 Block 统一使用 Agent SQLite，不保留内存后端。`CREATE BLOCK` 创建或打开持久数据，`CREATE TEMP BLOCK` 使用完整启动实例隔离的数据；均异步返回 `BlockOpened { block_id, created, temporary }`，相同 schema 再次打开不清空数据。GET 多来源使用同一读事务，INJECT 全部来源与目标使用同一写事务。TOOL/SANDBOX 在数据库中只保存完整 ResourceID，读取后按命令开始时取得的 owned 导入快照解析当前定义；已提交领域任务保持自身快照。MESSAGE 保留原始 typed 消息及 Inject 容器。`history_append` 只写历史，不修改 Block。旧 effect `finish` 已拒绝，只接受 `finish_turn`。

本轮 Driver 示例调整限于仓库根 Driver；驱动目录中的 Driver 未修改。详细设计仍按 lib、system、handler、events、types、error 六板块组织。

## 安装与执行

`MclPlugin::default()` 在 `RuntimePlugin::UPDATE` 注册唯一三阶段 System，需要安装 `AsyncRuntimePlugin`。插件使用共享 `Agent`，不依赖 Agent/Tool/Lua 插件的实现模块。Agent 启动合并共享 `MclEnvironmentFactory::build()`，factory 只提供 `mcl`；`wait`/`mcl_wait` 由 Lua 运行时提供，不进入 MCL 队列。

每个目标 Agent 一个 FIFO。唯一 System 第一阶段受理请求，第二阶段收口异步结果，第三阶段推进队首；队首推进预算每 Agent 每帧 64 条。pending 未完成时只阻塞同一 Agent。共享 `MclAsyncPhase` 区分 Completed、PreparedEffect、PreparedImport；准备结果回主阶段直接继续动作，不重新解析队首、不重复审计。effect 的所有独立选择器参数统一一次数据库 Get 请求，在同一读事务完成后才提交领域操作；纯 alias/manual effect 不额外读取 Block，但 GET 始终使用异步数据库入口。

停止或来源取消会取消未执行项并请求在途取消，仍等待实际 worker join 或执行方 oneshot。未开始项的取消审计使用异步批次，共享 `MclCancelledOutput` 收口；队列、pending 为空、`cancel_audits` 为零且全部回执已派发才通知 drained。准备活动在实际数据库操作前登记 Other，真实 join 后才标记完成；准备到领域阶段沿用同一取消 token。

Driver 回执通过内部 oneshot **立即发送**，然后才发布 `MclCommandResult` 观察事件和移除队首。公开 Result 不是 Driver 回复通道。ready 仅允许当前 Driver 对本人调用，核对完整 Entity 和启动关联；客户端或其他 Driver 不能替另一个 Agent ready。

## 指令与数组

```text
IMPORT MESSAGE prompt:local/soul AS soul
IMPORT TOOL tool:local/demo AS demo
IMPORT SANDBOX sandbox:local/write AS write
CREATE BLOCK b (messages MESSAGE, visible TOOL, activated SANDBOX)
CREATE TEMP BLOCK entry (pending MESSAGE)
GET soul, b.messages[0,2], b.messages[]
INJECT soul TO b.messages
INJECT demo TO b.visible
INJECT write TO b.activated
INJECT ? TO b.messages[-1]
INJECT [] TO b.messages
EMIT EFFECT inference (b.messages) (b.visible)
EMIT EFFECT tool_call ? (b.visible)
EMIT EFFECT history_append ?
EMIT EFFECT sandbox_use (b.activated)
EMIT EFFECT wake
EMIT EFFECT ready
EMIT EFFECT start_turn
EMIT EFFECT finish_turn
```

字段只有三种 typed 数组：`MESSAGE`、`TOOL`、`SANDBOX`，拒绝旧 `RESOURCE` 和 `TOOL_CALL`。TOOL 是带本地 alias 的可调用资源快照，不是 Assistant 的 ToolCall 请求。GET 的 alias 与数据库 Block 选择器按方括号外逗号切分，同类型拼接；空区间也参与类型检查。单索引和空字段读取始终返回数组。INJECT 在全部来源快照、目标范围与类型检查完成后，只做一次范围替换；错误不修改字段。

范围左闭右开，负端点使用 `len + 1`。`len=4` 时读取 `[-1]` 为 `[3,4]`，写入 `[-1]` 为 `[4,4]` 尾部插入；`INJECT [] TO b.f` 清空字段，而目标 `b.f[]` 只替换空区间。

## Driver 手动参数

`mcl(target_resource_id, text, arguments)` 的 arguments 是外层数组，每项为对应一个问号的完整数组，不能根据层数猜测或自动包装对象。零问号可省略 arguments。

```text
mcl("agent:local/demo", "EMIT EFFECT inference ? ?", [messages_array, ["demo"]])
mcl("agent:local/demo", "INJECT ? TO b.messages", [messages_array])
```

上述参数展示为结构伪代码；Lua 数组由运行时桥接为共享 `LuaValue::Array`。Lua 空 table 可能桥接为 `Object{}`，MCL host 仅在已明确的数组上下文中将空外层 table 转成零参数，将某一手动参数项的空 table 转成一个空数组；不会递归猜测消息对象或数组层数，非空对象仍拒绝，客户端原始 JSON 不变。`ToolCall` 元素结构是 `{id, tool_name, arguments}`，其中 arguments 为工具 JSON 参数**字符串**，调用 ID 批次内唯一。消息遵循共享 AgentMessage 的 system/inject/user/assistant/tool_call/error 结构。手动 TOOL/SANDBOX 数组只接受已导入 alias 字符串，通过 Agent 绑定读取可信快照；不接受裸 ResourceID 或完整 definition/schema/script/policy 对象。

## 异步与历史

IMPORT、inference、tool_call 通过领域请求共享一次性 Sender 槽、MCL 独占 Receiver。当前等待实现是 managed async service，内部监督等待任务并发送带原请求/pending 的收口事件，**不是** `ClosureRequest` 自动结果协议。资源加载登记 Other 活动并移交加载方；推理和工具执行方负责实际活动登记。

IMPORT 资源必须精确属于 driver_dependencies；无 builtin 隐含例外。声明类型必须匹配资源种类，alias 不能包含点、允许连字符。同 Agent alias 跨类别唯一，同 ID 重命名移除旧 alias；冲突在提交 loader 前检查，并在 loader 主线程收口再次检查，挂载失败回滚绑定。IMPORT 仅加载与绑定，不自动注入或执行。工具本次可见集合来自 TOOL 数组中的不可变快照，alias 冲突在准备阶段拒绝；每项执行失败或不可见结果均保留输入调用 ID 和顺序。推理结果保留 usage 和 stop_reason。

tool_call 第一参数必须恰好一条 Assistant，直接从其 tool_calls 提取请求；不再使用 work.calls 或 TOOL_CALL 字段。调用 ID 与名称非空、批次 ID 唯一，可见 TOOL alias 冲突整批拒绝。空调用同步返回 typed MESSAGE 空数组且不提交任务。TOOL 仅允许 tool 资源；旧 skill/hook ID 在导入时拒绝。

数据库 Block、指令审计及显式消息历史均在阻塞工作线程执行，不在主 ECS 做 SQLite I/O。除合法 history_append 外，指令历史在执行前单独事务写入，失败不执行。message 历史完全由显式 history_append 控制；推理和工具只返回结果，不自动追加 assistant、工具成功或失败消息。显式追加失败返回 history_append 错误，不重新执行原推理或工具。此机制不是跨网络、Block 与历史的全局事务。System 消息保留上下文角色，但 history_append 会递归拒绝直接或嵌套 Inject 中的 System。输入 Assistant 和工具结果均不自动写 message 历史；需要保存时由 Driver 显式追加。

资源值的宿主/网络输出采用安全投影，不发送完整执行材料或沙箱策略；目标数据库 Block 只保留资源ID，下一次读取按当前导入值解析；领域执行请求仍保留提交时选定快照，不随重载变化。

共享错误 `StorageCommitUnknown` 表示提交事实不确定，不伪装为 Cancelled 或确认回滚；worker 已退出且事务已结束时可以完成 Other。`StorageCleanupFailed` 表示清理尚未解决，保留并标记失败的 Other 活动，关闭作用域和新命令受理；作用域等待与队列 drain 等待返回清理失败，不能假称停止成功。取消审计批次的 CleanupFailed 也通过 drain 错误收口，回执仍携带 `history_error`，不是无限等待。

长期后台 task 协议尚未支持。cleanupTasks 保留共享 World API，不是 MCL 指令。未来扩展必须另定合同，不能隐式恢复旧语法。

## 验证

```sh
cargo test -p mcl_plugin
```

当前 MCL 测试共 35 项通过（1 项单元、3 项解析、1 项宿主参数、26 项调度、4 项 typed 数组）。覆盖显式 typed IMPORT、alias/selector 拼接及混型空数组、同 ID 重命名与数据库 ID 读取当前定义、MESSAGE 原容器、创建/再次打开、TEMP bool、旧 finish 拒绝、typed 空数组、负索引及失败 INJECT 不修改保证；也覆盖 FIFO、真实推理/工具/IMPORT oneshot、ready 完整身份与回执/drained 时序、source/stop 取消、真实数据库 worker 阻塞期间保留 Other 活动、effect 全参数一次读取、审计不重复、无隐式 message 历史及未解决清理失败的 scope/drain 错误。
