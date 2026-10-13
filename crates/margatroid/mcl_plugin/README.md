# MCL Plugin

新 MCL 的当前实现说明见 [DESIGN-NEXT.md](<DESIGN-NEXT.md>)。[DESIGN.md](<DESIGN.md>) 仅归档重写前 revision `7c9055c` 的旧实现；旧 REF/MERGE/BIND/LOAD/EXPOSE、visibility 与 history_record 路径已删除，不再受理。

## 安装与执行

`MclPlugin::default()` 在 `RuntimePlugin::UPDATE` 注册唯一三阶段 System，需要安装 `AsyncRuntimePlugin`。插件使用共享 `Agent`，不依赖 Agent/Tool/Lua 插件的实现模块。Agent 启动合并共享 `MclEnvironmentFactory::build()`，factory 只提供 `mcl`；`wait`/`mcl_wait` 由 Lua 运行时提供，不进入 MCL 队列。

每个目标 Agent 一个 FIFO。第一阶段受理请求，第二阶段收口异步结果，第三阶段推进队首，每 Agent 每帧同步预算 64 条。pending 未完成时只阻塞同一 Agent。停止取消未执行尾部并请求在途取消，仍等待实际执行方 oneshot 收口；队列和 pending 为空、所有回执已派发才通知 drained。

Driver 回执通过内部 oneshot **立即发送**，然后才发布 `MclCommandResult` 观察事件和移除队首。公开 Result 不是 Driver 回复通道。ready 仅允许当前 Driver 对本人调用，核对完整 Entity 和启动关联；客户端或其他 Driver 不能替另一个 Agent ready。

## 指令与数组

```text
IMPORT MESSAGE prompt:local/soul AS soul
IMPORT TOOL tool:local/demo AS demo
IMPORT SANDBOX sandbox:local/write AS write
CREATE BLOCK b (messages MESSAGE, visible TOOL, activated SANDBOX)
GET soul, b.messages[0,2], b.messages[]
INJECT soul TO b.messages
INJECT demo TO b.visible
INJECT write TO b.activated
INJECT ? TO b.messages[-1]
INJECT[] TO b.messages
EMIT EFFECT inference (b.messages) (b.visible)
EMIT EFFECT tool_call ? (b.visible)
EMIT EFFECT history_append ?
EMIT EFFECT sandbox_use (b.activated)
EMIT EFFECT wake
EMIT EFFECT ready
EMIT EFFECT start_turn
EMIT EFFECT finish
```

字段只有三种 typed 数组：`MESSAGE`、`TOOL`、`SANDBOX`，拒绝旧 `RESOURCE` 和 `TOOL_CALL`。TOOL 是带本地 alias 的可调用资源快照，不是 Assistant 的 ToolCall 请求。GET 的 alias 与普通 Block 选择器按方括号外逗号切分，同类型拼接；空区间也参与类型检查。单索引和空字段读取始终返回数组。INJECT 在全部来源快照、目标范围与类型检查完成后，只做一次范围替换；错误不修改字段。

范围左闭右开，负端点使用 `len + 1`。`len=4` 时读取 `[-1]` 为 `[3,4]`，写入 `[-1]` 为 `[4,4]` 尾部插入；`INJECT[] TO b.f` 清空字段，而目标 `b.f[]` 只替换空区间。

## Driver 手动参数

`mcl(target_resource_id, text, arguments)` 的 arguments 是外层数组，每项为对应一个问号的完整数组，不能根据层数猜测或自动包装对象。零问号可省略 arguments。

```text
mcl("agent:local/demo", "EMIT EFFECT inference ? ?", [messages_array, ["demo"]])
mcl("agent:local/demo", "INJECT ? TO b.messages", [messages_array])
```

上述参数展示为结构伪代码；Lua 数组由运行时桥接为共享 `LuaValue::Array`。Lua 空 table 可能桥接为 `Object{}`，MCL host 仅在已明确的数组上下文中将空外层 table 转成零参数，将某一手动参数项的空 table 转成一个空数组；不会递归猜测消息对象或数组层数，非空对象仍拒绝，客户端原始 JSON 不变。`ToolCall` 元素结构是 `{id, tool_name, arguments}`，其中 arguments 为工具 JSON 参数**字符串**，调用 ID 批次内唯一。消息遵循共享 AgentMessage 的 system/inject/user/assistant/tool_call/error 结构。手动 TOOL/SANDBOX 数组只接受已导入 alias 字符串，通过 Agent 绑定读取可信快照；不接受裸 ResourceID 或完整 definition/schema/script/policy 对象。

## 异步与历史

IMPORT、inference、tool_call 通过领域请求共享一次性 Sender 槽、MCL 独占 Receiver。当前等待实现是 managed async service，内部监督等待任务并发送带原请求/pending 的收口事件，**不是** `ClosureRequest` 自动结果协议。资源加载登记 Other 活动并移交加载方；推理和工具执行方负责实际活动登记。

IMPORT 资源必须精确属于 image_dependencies；无 builtin 隐含例外。声明类型必须匹配资源种类，alias 不能包含点、允许连字符。同 Agent alias 跨类别唯一，同 ID 重命名移除旧 alias；冲突在提交 loader 前检查，并在 loader 主线程收口再次检查，挂载失败回滚绑定。IMPORT 仅加载与绑定，不自动注入或执行。工具本次可见集合来自 TOOL 数组中的不可变快照，alias 冲突在准备阶段拒绝；每项执行失败或不可见结果均保留输入调用 ID 和顺序。推理结果保留 usage 和 stop_reason。

tool_call 第一参数必须恰好一条 Assistant，直接从其 tool_calls 提取请求；不再使用 work.calls 或 TOOL_CALL 字段。调用 ID 与名称非空、批次 ID 唯一，可见 TOOL alias 冲突整批拒绝。空调用同步返回 typed MESSAGE 空数组且不提交任务；Hook 导入合法但执行语义未支持，明确返回失败。

除合法 history_append 外，指令历史在执行前单独事务写入，失败不执行。结果历史在真实执行后单独批次事务追加，失败附 history_error，不覆盖真实结果或重试工具。此机制不是跨网络、Block 与历史的全局事务。System 消息保留上下文角色，但 history_append 会递归拒绝直接或嵌套 Inject 中的 System。工具结果自动写历史，不重复记录输入 Assistant。

资源值的宿主/网络输出采用安全投影，不发送完整执行材料或沙箱策略；内部 Block 与领域请求仍保留选定资源快照，重载不会改变旧 Block 中的版本。

长期后台 task 协议尚未支持。cleanupTasks 保留共享 World API，不是 MCL 指令。未来扩展必须另定合同，不能隐式恢复旧语法。

## 验证

```sh
cargo test -p mcl_plugin
```

测试覆盖显式 typed IMPORT、alias/selector 拼接及混型空数组、同 ID 重命名与旧 Block 快照、空 Assistant 调用无任务、导入 alias 冲突无 loader 请求、严格语法、typed 空数组、负索引、INJECT 不修改保证、FIFO 和预算、真实推理/工具/IMPORT oneshot、停止取消、历史失败以及 ready 身份和回执/drained 时序。
