# DtoPlugin

DtoPlugin 是 WebSocket 传输与共享领域事件之间的转换层。协议值由 Protocol 提供，领域组件与事件由共享 types 提供；不依赖 Agent、MCL、Workspace、Client 或 Memory 插件实现。

## 当前路由

| 入站 type | 行为 |
| --- | --- |
| `connection.register` | 发送 ServerPlugin `RegisterConnection` |
| `workspace.start` | 转换共享 `StartWorkspace` 并保存网络回复关联 |
| `workspace.stop` | 受检解析完整 Workspace 后调用共享 World 扩展 |
| `mcl.command` | 解析当前成员或 manager，绑定真实注册 Client.resource_id，发送共享 MclCommand |
| `state.query` | 只读状态快照，直达请求连接 |
| `history.query` | 从当前 Agent.history.list 读取历史，直达请求连接 |

除注册外，未注册连接返回关联请求 ID 的 `request.failed`，不延后受理。注册成功回执仅在共享 Client 实体存在且身份匹配后发送。

MCL Result 按 id/source/target 匹配待回复记录，仅向发起连接发送。回复前再次检查完整 Client Entity generation 与 source，迟到结果不会发送给复用身份的新实例。Workspace 请求使用内部唯一 operation_id 关联领域结果，网络回复还原原始 id，防止不同 Client 的相同网络请求 ID 相互混淆。

断开连接时网络层待回复记录被清除；同时按连接存活和完整 Client Entity 核对并清除陈旧记录，包含 MCL、Workspace start/stop 三类。此清除只释放网络回复关联，不自动取消已提交的领域操作。清除后的迟到结果被忽略，不能重建回复记录。

## 出站和状态

`WebSocketMessageSend` 仅重新导出共享 types::dto_events 事件，其字段为共享 target 和 JSON Value，不携带 Protocol 的 ServerMessage。调用方先将 ServerMessage 序列化为 Value；DTO 按 Broadcast/Type/Name 筛选实际连接并发送 Text。

状态快照从 Workspace、Agent 与 Agent.history 读取，仅在状态内容或目标连接集合变化时发送，避免事件驱动 Runtime 的自发循环。历史 payload 保留结构化消息与 usage。共享 HistoryRecord 尚无时间与持久序号，协议投影 sequence 为列表顺序的 1-based 编号，created_at_ms=0 表示未知，不是真实时间。

日志订阅使用 TracingStream，经共享 JSON 事件发送到配置的 logs 目标。日志自身发送失败不再生成日志，订阅滞后不写回 tracing，避免递归。

## 边界与已归档行为

插件不创建领域 Entity、不执行 MCL、不操作 Driver、不负责鉴权或资源权限。身份绑定仅保证请求来源与连接一致，不等同完整认证。

旧 agent.message 邮箱、agent.turn.abort、visibility、Workflow 与 pending oneshot 轮询表已移除。旧 Memory/HistoryMessage、默认可见性、AgentTokenUsage 副本与消息广播报告不是当前接口。当前 pending 是网络请求回复关联，由共享结果事件驱动，不是 Agent 的执行 pending。
