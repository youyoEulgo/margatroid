# mcl_plugin

`mcl_plugin` 执行 [mcl_core](../core/README.md) 的命令事件，将消息操作与领域效果连接到 Agent、Storage、Inference、Tool 和资源 core。它负责来源核对、目标 FIFO、类型转换、异步等待、审计与一次直接回复。

## 核心对象

| 对象 | 职责 |
| --- | --- |
| `MclPlugin` | 安装命令 System，配置 schedule 与导入资源根目录 |
| `MclImportConfig` | prompt、tool、sandbox 三个资源根路径 |
| `MclQueue` | 每个目标的排队命令、执行锁与活动请求 |
| `CommandContext` | 一次命令持有的请求、操作、取消与领域调用材料 |
| 完成 guard | 直接回复、活动移除、drain 更新及执行锁释放 |
| `MclDrain` | 等待实际执行、审计及回复收口 |

公开安装 API 为 `MclPlugin::new()`、`with_schedule(...)` 和 `with_import_config(...)`；其余执行细节通过 core 命令及结果交接。

## 装配一次执行环境

下例使用公开安装方法配置运行与导入根目录。完整领域运行还需按命令类型装配 Storage、Inference、Tool 与相应资源处理者。

```rust
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin};
use core_plugin::App;
use mcl_plugin::{MclImportConfig, MclPlugin};

let mut app = App::new();
app.add_plugin(RuntimePlugin::default())
    .add_plugin(AsyncRuntimePlugin)
    .add_plugin(ClosurePlugin)
    .add_closure_system(RuntimePlugin::UPDATE)
    .add_plugin(MclPlugin::new().with_import_config(MclImportConfig {
        prompt_root: "resources/prompts".into(),
        tool_root: "resources/tools".into(),
        sandbox_root: "resources/sandboxes".into(),
    }));
```

默认 schedule 为 `UPDATE`，支持 `UPDATE` 与 `POST_UPDATE`；两者均在 `PRE_UPDATE` 中止控制之后。安装时检查 Runtime、AsyncRuntime、目标 schedule、闭包入口与单次安装条件。三个默认资源根路径与示例一致。

## 完整调用路径

[集成测试](../../compose/tests/mcl_commands.rs) 的 `Fixture` 展示了运行设施、Storage、目标 Agent、来源 Address、消息资源和回复驱动，可作为完整 Rust 接入样例。

1. 准备真实来源 Entity、来源绝对节点地址与目标 Agent，构造 `MclInstruction`，分配 id、取消令牌及 oneshot。
2. 发送 `MclCommand`；处理者校验来源 Address、可选宿主实例和目标 Starting/Running 状态，随后解析指令并加入目标队列。
3. 对 `CREATE TEMP BLOCK msg (context)` 执行创建，再以 `INJECT ? TO msg.context` 绑定用户消息数组，最后提交 `GET msg.context`。
4. Storage 回复后，执行方完成审计并通过原 oneshot 返回 `MclValue::Array(MessageArray::Message(...))`。
5. 应用关闭时调用 core 的 `mcl_close`，在 World 外等待 `mcl_drain(...).wait().await`。

文本与 JSON 参数的写法见 [领域示例](../README.md)，完整语法见 [上级 base.lua](../../../../../base.lua)。

## 领域交接

| 命令 | 交接内容与回复 |
| --- | --- |
| IMPORT | 校验 alias 和资源定位符，向对应资源 core 登记路径及元数据 |
| Block / CONFIG | Storage 执行文件、事务、设置与快照操作后回复 |
| inference | 转换消息，读取 Agent 参数，发送 `InferenceRequest` 并等待 oneshot |
| tool_call | 投影结构化调用，整批发送独立回复请求，按输入顺序收齐工具消息 |
| history_append | 显式存储完整内部消息，返回真实历史 sequence |
| ready / start_turn / finish_turn | 调用 Agent core 生命周期方法，finish_turn 等待本轮推理收口 |
| wake / sandbox_use | 操作 Agent wake_slot 或通过沙箱 core 选择已导入 alias |

Tools 中保存导入时的 alias、description、input_schema 与 path；推理使用当前完整定义，工具执行读取登记路径并按登记 schema 校验。Prompt alias 在消息读取时按路径加载。

省略 turn 为自由调用；显式 turn 交由领域实际受理入口校验。inference 的第三个参数为 tag，带 tag 时第二参数支持 `null`；普通推理使用 `inference`，压缩使用 `compact`，tag 随本次流投递。

## 回复与并发

解析、权限和执行错误转换为 `MclValue::Error`，消息携带原命令 request_id；推理领域错误转换为 `MclValue::Inference` 中的 error 消息及 `stop_reason=error`。工具批次逐项保留调用 ID，失败对应 failed 工具消息。

CONFIG、Block、IMPORT 与普通状态操作持队列锁至实际执行和审计完成；推理、工具及 finish_turn 等待期间继续分派后续命令。完成 guard 负责一次回复、活动记录移除及实际解锁唤醒。关闭收口覆盖排队操作、领域任务、文件 worker 和回执。

## 源码与测试导航

- [公开安装与 schedule 检查](src/lib.rs)、[命令 System](src/system.rs)。
- [受理、解析、领域执行与回复](src/handler.rs)、[执行上下文及完成 guard](src/types.rs)。
- [core 语法测试](../core/tests/syntax.rs)：参数、选择器、快照与 turn/tag。
- [集成测试](../../compose/tests/mcl_commands.rs)：导入、配置顺序、消息历史、快照、工具批次、自由调用与 drain。
