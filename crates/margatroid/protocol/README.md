# Margatroid Protocol

显式message历史策略已实现：推理和工具调用只返回结果，所有message历史仅由显式MCL history_append控制。MCL指令审计仍自动记录，审计payload中的消息不作为独立message记录。示例Driver对需要保留的结果另发显式追加指令。


`margatroid_protocol` 定义 JSON DTO、纯转换以及只读、受检的 Workspace/Agent 逻辑寻址。领域组件、消息和指令来自共享 types，不依赖 Agent、MCL、Workspace 或 Memory 插件实现。

## 请求与身份

客户端信封为 `{ "type": "...", "id": "...", "message": {...} }`。支持：

| type | message |
| --- | --- |
| `connection.register` | `{ client_type, name? }` |
| `workspace.start` | `{ definition }` |
| `workspace.stop` | `{ workspace }` |
| `mcl.command` | `{ workspace, agent?, command, arguments }` |
| `state.query` | `{}` |
| `history.query` | `{ workspace, agent? }` |

除注册外必须已经注册。错误回执为 `request.failed { id, error: { kind, message } }`，错误正文最多 512 UTF-8 字节。注册成功为 `connection.registered { id, client: { resource_id, client_type, name } }`；失败为 `connection.register_failed { id, error }`。等待注册回执后再提交请求。

`workspace` 为 `{ id, name, project_root }`，三者必须与当前 Workspace 一致。`agent: null` 选择成员定义中的 manager；其他值必须是该 Workspace 当前成员的规范化资源地址。入站不接受 Entity index；出站 `agent_instance` 是包含 generation 的不透明实例标识，不授予寻址能力。

Workspace definition 字段为 `id, name, project_root, manager, agents`。成员字段为 `id, name, image, home_root, history_path`，不含旧资源可见性、Workflow 或 memory 配置。

## MCL

```json
{
  "type": "mcl.command",
  "id": "mcl-1",
  "message": {
    "workspace": { "id": "workspace:local/demo:latest", "name": "demo", "project_root": "/project/demo" },
    "agent": null,
    "command": "GET msg.recent_conversation",
    "arguments":[]
  }
}
```

每个 `arguments` 项必须是数组，按顺序用于问号参数。一次请求仅一条 MCL 指令，不接受 Lua 源码。`source` 由 DTO 入口读取真实注册 Client 的资源地址，客户端不能提供。

结果仅发送到发起连接：`mcl.command_result { id, source, target, result, history_error }`。`result` 保留 Rust Result 的 `{"Ok": ...}` / `{"Err": {kind,message}}` 形状及 MclValue 类型标签。Inference 结果保留 `stop_reason`、完整 assistant 和 usage；`history_error` 仅报告取消等指令审计附带失败；message 历史需要另发显式 history_append，失败通过该请求的 result 返回，不表示需要重做推理或工具。

Rust 构造 API：`ClientMessage::mcl_command(id, &workspace, agent, command, Vec<Value>)`，最后一项是 arguments，不是旧 binding。

## 消息、状态与历史

`AgentMessageDto`（亦可用 `MessageDto` 名称）是 snake_case `type` 标签的递归消息值：`inject`、`user`、`assistant`、`tool_call`、`error`。Inject 可递归包含全部同类消息；转换限制深度 32、节点 4096、序列化大小 1 MiB，不采用旧内层白名单。Assistant 包含可选 usage；ToolCall 的 resource_id 可为 null。允许消息形状不表示网络入口有权伪造工具完成事实。

`state.sync { state }` 包含 `workspaces, agents, histories`。Agent 快照包含 `workspace, agent, agent_instance, status, turn, sandbox, queued_commands, pending_command`；status 为 creating/running/stopping。历史来自 `Agent.history.list()`，不是旧实时 Memory/HistoryMessage 接口。

历史消息字段为 `key, sequence, turn, kind, content, payload, created_at_ms`，payload 保留结构化消息、usage、命令及参数。**当前共享 HistoryRecord 不含持久序号与时间：sequence 是 list 顺序的 1-based 投影，created_at_ms 固定为 0，表示未知，不是真实创建时间。** `history.result { id, history }` 返回相同历史投影。

推理流使用 `agent.message.delta` / `agent.message.reasoning_delta`，字段为 `id, agent, agent_instance, sequence, content`。id 是推理请求 ID，不是 turn。`agent.message.finished { id, agent, agent_instance, error }` 标记流终态；流及终态仅供渲染，MCL Result/领域 oneshot 才是操作完成权威。

## 已归档的旧协议

旧 `agent.message` 邮箱、`agent.turn.abort`、visibility、Workflow 入口及 `binding` 均已移除。旧 working、默认/动态可见性、AgentTokenUsage 副本与完整消息作为流完成标志的描述不再适用；本页为当前实现契约，旧行为仅属历史设计。
