# Margatroid Protocol

显式message历史策略已实现：推理和工具调用只返回结果，所有message历史仅由显式MCL history_append控制。MCL指令审计仍自动记录，审计payload中的消息不作为独立message记录。示例Driver对需要保留的结果另发显式追加指令。


`margatroid_protocol` 定义 JSON DTO、纯转换以及只读、受检的 Workspace/Agent 逻辑寻址。领域组件、消息和指令来自共享 types，不依赖 Agent、MCL、Workspace 或 Memory 插件实现。

## 用户轮次中止目标（待实现）

本节为已批准的待实现契约，不声称当前代码已支持；不是复活旧邮箱、中止查表或全局 pending 实现。无 `get_turn`。

- 请求：`{ "type":"agent.turn.abort", "id":"abort-1", "message":{ "workspace":{id,name,project_root}, "agent":null, "turn_id":"turn-1" } }`。`id` 为网络请求关联，`turn_id` 为必填非空的预期活动轮次；agent 为 null 时选择 manager，其余身份规则与 MCL 相同。
- 回执：`agent.turn.abort_result { id, workspace, agent, turn_id, result }`；result 使用 `{"Ok":null}` / `{"Err":{kind,message}}`。Ok 仅确认中止已受理或该关联轮次已中止，不证明执行 cleanup 已结束；失败保留请求 id。仅回复发起连接，source 来自真实注册 Client。
- 入口受检解析 Workspace 与当前成员的完整实例，实际处理再次核对 turn_id；旧/不匹配轮次不得取消新轮次。对同一已中止轮次（含 Working 且 turn=None）重复请求幂等成功，仅保留必要已中止关联，不恢复全局 pending 表；未识别轮次报关联错误。
- 状态为 `creating/idle/working/stopping`。中止立即清空 turn，真实清理期间保持 Working，正常 finish 收口才 Idle；状态同步同时发布 turn 与 status。前端据 status 显示运行态、据 turn 关联中止，不能把回执或 turn=None 当作 Idle。
- AbortSystem 必须先于 MCL 实际执行及 inference/tool admission。共享事件允许各消费者独立观察，不靠 take reply 独占中止通知；执行前再次核对请求轮次，turn=None 或已中止旧轮次不启动推理/工具而返回正常取消结果。
- Assistant 新增 stop_reason，覆盖全部推理原因，新增 `StopReason::Cancelled` 序列化 `cancelled`。兼容保留 `InferenceResponse.stop_reason`，单一权威映射生成，两处始终相同。用户轮次中止且真实清理成功返回 Ok assistant：`content="Turn aborted."`、`reasoning=None`、`tool_calls=[]`、`usage=None`、`stop_reason=Cancelled`。
- 取消工具且清理成功返回 Ok ToolCall：`failed=true`、`content="Tool call aborted."`，保留调用 ID、名称及已知 resource_id。批次保留已完成真实结果，其余每个调用（包括未启动项）各有一个原序结果；不虚构部分执行成功，不撤销已发生副作用。
- `CleanupFailed` 仍为 Err，保留活动，不伪造取消或 Idle。用户轮次中止不同于根 Agent shutdown、owner abort、timeout、receiver closed，不统一转换这些原因。
- 历史只经显式 history_append 保存；Assistant stop_reason 存入 payload 元数据而非 content。cancelled assistant 可入历史，不得进入 infer 上下文（包括 Inject 嵌套）。

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

`AgentMessageDto`（亦可用 `MessageDto` 名称）是 snake_case `type` 标签的递归消息值：`inject`、`user`、`assistant`、`tool_call`、`error`。Inject 可递归包含全部同类消息；转换限制深度 32、节点 4096、序列化大小 1 MiB，不采用旧内层白名单。目标 Assistant 包含 stop_reason 和可选 usage；ToolCall 的 resource_id 可为 null。允许消息形状不表示网络入口有权伪造工具完成事实。

`state.sync { state }` 包含 `workspaces, agents, histories`。Agent 快照包含 `workspace, agent, agent_instance, status, turn, sandbox, queued_commands, pending_command`；当前实现 status 为 creating/running/stopping；待实现目标改为 creating/idle/working/stopping，见上文。历史来自 `Agent.history.list()`，不是旧实时 Memory/HistoryMessage 接口。

历史消息字段为 `key, sequence, turn, kind, content, payload, created_at_ms`，payload 保留结构化消息、usage、命令及参数；目标新增 stop_reason 元数据，不能混入 content。**当前共享 HistoryRecord 不含持久序号与时间：sequence 是 list 顺序的 1-based 投影，created_at_ms 固定为 0，表示未知，不是真实创建时间。** `history.result { id, history }` 返回相同历史投影。

推理流使用 `agent.message.delta` / `agent.message.reasoning_delta`，字段为 `id, agent, agent_instance, sequence, content`。id 是推理请求 ID，不是 turn。`agent.message.finished { id, agent, agent_instance, error }` 标记流终态；流及终态仅供渲染，MCL Result/领域 oneshot 才是操作完成权威。

## 已归档的旧协议

旧 `agent.message` 邮箱、旧 `agent.turn.abort` 查表实现、visibility、Workflow 入口及 `binding` 均已移除。上文同名中止控制是新的待实现契约，不复活旧实现；目标 Working 状态也不恢复旧生命周期。默认/动态可见性、AgentTokenUsage 副本与完整消息作为流完成标志的旧描述仍不适用。
