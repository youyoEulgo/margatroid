# mcl_core

`mcl_core` 提供 MCL 解析器、语法树、命令事件、结果值与队列生命周期契约。Rust 宿主可直接解析指令，也可向 [mcl_plugin](../plugin/README.md) 发送 `MclCommand` 完成领域执行。

包依赖为 mecs `core_plugin`、Serde、TOML 与 Tokio。完整命令语法见 [上级 base.lua](../../../../../base.lua)。

## 核心对象

| 对象 | 用途 |
| --- | --- |
| `MclInstruction` | `text: String` 和 `arguments: Vec<Value>` |
| `MclOperation` | Import、Create、Get、Inject、Config、Effect 语法树 |
| `Selector` / `Selection` | Block 字段及 All、Index、Range 选择 |
| `MessageExpression` | 按顺序组合 Prompt alias 与 Block selector |
| `MclCommand` / `MclReply` | 来源与目标、取消、执行关联及 oneshot |
| `MclValue` / `MessageArray` | 领域结果与结构化消息数组 |
| `MclQueue` / `MclDrain` | 接收队列、执行锁、取消与真实收口 |

`parse_instruction(&MclInstruction)` 返回 `Result<MclOperation, MclError>`；`parse_selector(&str)` 与 `validate_identifier(&str)` 提供选择器和名称校验。

## 从输入到语法树

下面的公开 API 示例取自 [语法测试](tests/syntax.rs) 的 CONFIG 参数用例，完成参数绑定、解析及类型核对。

```rust
use mcl_core::{parse_instruction, ConfigOperation, MclInstruction, MclOperation};
use serde_json::json;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let instruction = MclInstruction {
        text: "CONFIG SET ? ?".into(),
        arguments: vec![json!("compaction.threshold"), json!(0.8)],
    };
    let operation = parse_instruction(&instruction)?;
    let MclOperation::Config(ConfigOperation::Set { path, value }) = operation else {
        panic!("expected a config update");
    };
    assert_eq!(path, "compaction.threshold");
    assert_eq!(value, toml::Value::Float(0.8));
    Ok(())
}
```

解析器检查单条指令、问号数量、字段名称、范围及效果参数形状。范围端点要求同号并有序；索引从 0 开始，`-1` 表示末项。领域资源读取、权限、消息校验及执行由 plugin 接续。

## 命令与结果接入

一次执行需要为 `MclCommand` 填充 `id`、`source`、`source_address`、`source_instance`、`target`、`instruction`、`cancel` 和 `reply`。来源地址与实际来源实体的 Address 对应，`source_instance` 可表达已绑定的宿主实例。

`MclReply` 为 `Arc<Mutex<Option<oneshot::Sender<Result<MclValue, MclError>>>>>`。宿主发送事件后，运行循环驱动插件并等待 receiver；完整装配和发送写法见 [集成测试](../../compose/tests/mcl_commands.rs)。

| 结果 | 序列化形状示例 |
| --- | --- |
| 消息数组 | `{"Array":{"Message":[]}}` |
| 领域推理 | `{"Inference":{"message":{},"stop_reason":"completed"}}` |
| 可恢复命令错误 | `{"Error":{"type":"error","message":"…","request_id":"…"}}` |
| 回合 | `{"Turn":"turn:one"}` |

其余结果包括 `Unit`、`Config`、`BlockSnapshots` 和 `Import`。`MclValue` 中的消息为 JSON 值，执行插件负责与相应领域消息类型转换。

## 关闭与收口

`WorldMclExt::mcl_close(target)` 将目标队列设为停止受理，并触发共享取消令牌；`mcl_drain(target)` 返回可克隆的 `MclDrain`。调用方在 World 外执行 `drain.wait().await` 等待 outstanding 归零，实际清理错误随结果返回。

`MclQueue` 跟踪 FIFO、活动命令与执行锁。`MclDrain::started()`、`completed(error)` 及 `wait()` 为执行方提供计数和等待契约。

## 源码与测试导航

- [公开导出、解析器与 World 扩展](src/lib.rs)。
- [指令、语法树与结果](src/types.rs)。
- [命令与回复事件](src/events.rs)、[队列组件](src/components.rs)、[错误分类](src/error.rs)。
- [语法与序列化测试](tests/syntax.rs)：CONFIG、Block、选择器、快照、turn/tag。
- [完整执行测试](../../compose/tests/mcl_commands.rs)：真实来源、领域回复、队列关闭和历史。
