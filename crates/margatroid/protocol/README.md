# margatroid_protocol

`margatroid_protocol` 定义 Margatroid 的 JSON 消息格式，提供请求解析、服务端编码和字段校验。开发者可以用它编写客户端、实现网络入口，或为消息构造契约测试。

## 读懂一次交互

客户端请求使用三个字段：`type` 指定操作，`id` 关联回复，`message` 保存该操作的参数。

```json
{
  "type": "agent.turn.req",
  "id": "turn-1",
  "message": { "address": "/demo/assistant" }
}
```

服务端通过对应的响应类型交付查询结果，通过 `*.sync` 消息交付状态变化。响应携带请求ID；同步消息按地址、记录序号或生成ID关联来源。

通常先发送 `connection.register` 建立客户端身份，再查询Workspace和Agent，随后发送MCL或生命周期操作。执行与实体查找由 [DTO层](../dto/README.md)完成。

## 在Rust中解析请求

```rust
use margatroid_protocol::parse_client_message;

fn main() -> Result<(), margatroid_protocol::ProtocolError> {
    let request = parse_client_message(
        r#"{"type":"workspace.req","id":"list-1","message":{}}"#,
    )?;
    assert_eq!(request.id(), "list-1");
    Ok(())
}
```

`parse_client_message` 返回 `ClientMessage`，先完成JSON结构和对应参数校验。服务端构造 `ServerMessage` 后，通过 `encode_server_message` 取得经过校验的JSON字符串。两种枚举都支持Serde，网络入口使用上述函数统一执行校验。

## 核心类型

| 类型 | 用途 |
| --- | --- |
| `ClientMessage` | 连接注册、配置、资源、Workspace、Agent和MCL请求 |
| `ServerMessage` | 请求响应与主动同步 |
| `AgentMessageDto` | system、user、assistant、compact、tool_call、inject和error消息 |
| `WorkspaceInfoDto` | Workspace状态及持久Agent登记 |
| `AgentTaskDto` | 活跃任务与保留输出窗口 |
| `AgentUiBundleDto` | 带revision和文件内容的Driver UI资源集合 |
| `ProtocolError` / `ProtocolErrorDto` | Rust错误与网络错误载荷 |

完整变体和字段集中在 [types.rs](src/types.rs)。节点地址使用 `/demo/assistant` 这类绝对路径，文件资源地址使用 `tool:margatroid/task-list:dev` 这类完整标识。

## 流式生成与历史记录

`agent.message.chunk.sync` 携带同一 `generation` 的累计正文和reasoning。接收方按 `sequence` 更新该生成的显示内容。`agent.message.full.sync` 交付终态，用完整消息校正生成内容。

`turn` 表示显式回合关联，`tag` 表示执行用途。Assistant普通推理使用 `inference`，压缩摘要使用 `compact`；自由调用的turn和省略的标签分别以null表示。相同generation沿用同一tag。

持久消息使用实际 `history_sequence`，并与消息的 `sequence` 对应。生成full的history_sequence为null；持久消息full的generation和tag为null。历史、MCL及日志分页使用排他序号边界，结果按序号升序交付。

## 查阅接口

- [解析、编码与UI资源校验](src/lib.rs)
- [消息和DTO定义](src/types.rs)
- [协议错误](src/error.rs)
- [请求与响应示例](tests/protocol.rs)
- [执行标签示例](tests/tags.rs)
- [实现设计](DESIGN.md)
