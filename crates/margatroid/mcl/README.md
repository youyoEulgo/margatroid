# MCL

MCL 为 Agent 提供逐条执行的消息操作与领域效果命令。Driver、工具宿主及网络入口可通过同一个 `MclCommand` 执行导入、Block 读写、设置更新、推理、工具调用与回合控制。

完整语法参考为项目上级的 [base.lua](../../../../base.lua)。本页介绍领域对象、调用路径与开发入口；Rust 类型及装配分别见 [core](core/README.md) 和 [plugin](plugin/README.md)。

## 核心对象与职责

| 对象 | 职责 | 定义 |
| --- | --- | --- |
| `MclInstruction` | 一条文本指令及按顺序绑定的 JSON 参数 | [core 类型](core/src/types.rs) |
| `MclOperation` | 解析后的 Import/Create/Get/Inject/Config/Effect | [解析器](core/src/lib.rs) |
| `Selector` / `MessageExpression` | Block 字段、索引范围及有序消息来源 | [core 类型](core/src/types.rs) |
| `MclCommand` | 真实来源、目标、取消与唯一回复 | [事件](core/src/events.rs) |
| `MclValue` | Unit、Turn、消息数组、Config、Inference 等结果 | [core 类型](core/src/types.rs) |
| `MclQueue` / `MclDrain` | 目标 FIFO、执行锁和关闭收口 | [core README](core/README.md) |
| `MclPlugin` | 领域转换、异步执行、审计和直接回复 | [plugin README](plugin/README.md) |

core 依赖 mecs 与通用库，消息使用结构化 JSON 值；plugin 将值校验并转换为所属领域类型，连接 Agent、Storage、资源、Inference 和 Tool 的 core 契约。

## 领域操作矩阵

| 类别 | 输入与作用 | 结果 |
| --- | --- | --- |
| IMPORT | 资源定位符 `type:scope/name:tag` 与 alias | `Import` |
| CREATE / GET / INJECT | 消息 Block、来源表达式及选择器 | `Unit` / `Array.Message` |
| CONFIG | 当前 Agent 的点分键路径与 TOML 值 | `Config` / `Unit` |
| inference | 消息、可选显式 turn、可选 tag | `Inference { message, stop_reason }` |
| tool_call | 调用数组或消息的 tool_calls 投影、可选 turn | 按输入顺序排列的 `Array.Message` |
| history_append | 完整内部消息数组 | 带真实 sequence 的 `Array.Message` |
| block_snapshot / block_restore | Block 名称及恢复用快照 ID | `Unit` |
| block_snapshots | Block 名称 | `BlockSnapshots` |
| start_turn / finish_turn | 当前 Agent 回合 | `Turn` / `Unit` |
| ready / wake / sandbox_use | 生命周期、唤醒及沙箱 alias 数组 | `Unit` |

持久 Block 的 timeline 及快照由 Storage 管理；TEMP Block 与快照驻留内存并按 Agent.instance 隔离。GET 按来源顺序拼接，单索引结果同样为数组；INJECT 的整体、插入及范围行为见语法参考。

## 一次完整调用

示例使用已安装Storage与Inference处理者的应用，目标Agent先通过 `EFFECT model_set ?` 配置有效模型路由。随后逐条提交以下指令，创建临时上下文、写入用户消息并推理；每条指令对应一次 `MclInstruction` / `MclCommand`。

```text
CREATE TEMP BLOCK msg (context)
  arguments: []
INJECT ? TO msg.context
  arguments: [[{"type":"user","content":"请回答 hello"}]]
GET msg.context
  arguments: []
EFFECT inference (msg.context) ? ?
  arguments: [null, "inference"]
```

调用方以自身真实 Entity 和完整节点地址填充来源，提供目标 Agent、取消令牌与 oneshot。plugin 核对来源及目标状态，解析后进入目标 FIFO，再把 inference 交给推理领域；调用方收到 `MclValue::Inference` 后，可将其 `message` 作为 `EFFECT history_append ?` 的消息数组输入。

可运行的 Rust 装配、事件提交、tick 和结果核对见 [MCL 集成测试](../compose/tests/mcl_commands.rs)。Lua 宿主的调用形态为 `margatroid.mcl:execute(target_address, text, arguments)`，其中 target 为 `/demo/coder` 等绝对节点地址。

## 关联、结果与关闭

省略 turn 表示自由调用；显式 turn 由推理和工具实际受理入口校验。带 tag 的三参数 inference 支持以 `null` 表示自由调用，普通推理 tag 为 `inference`，压缩为 `compact`。tag 作用于本次推流顶层投递元数据。

命令解析、权限与执行错误经回复转换为 `MclValue::Error`，内含 `type=error`、message 和原 request_id。推理领域错误位于 `Inference.message`，`stop_reason=error`；工具批次中的各调用错误位于对应 failed 工具消息。

CONFIG、Block、IMPORT 及普通状态效果持执行锁至实际执行与审计结束；推理、工具和 finish_turn 异步等待期间允许后续命令分派。关闭时 `mcl_close` 停止受理并触发取消，`mcl_drain` 提供真实执行和回复收口的等待句柄。

## 接口与测试导航

- [core 接入](core/README.md)：解析、请求、结果与关闭扩展。
- [plugin 接入](plugin/README.md)：运行依赖、资源根目录与领域调用。
- [语法测试](core/tests/syntax.rs)：参数数量、选择器、turn/tag 与结果形状。
- [完整领域测试](../compose/tests/mcl_commands.rs)：消息、导入、历史、快照、工具批次与关闭。
