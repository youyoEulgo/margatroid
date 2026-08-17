# AgentPlugin

`AgentPlugin`只拥有Agent实例的Workspace关联、上下文、默认/动态可见性、轮次状态和累计Token状态。它消费
`AgentCreateRequest`创建Agent自身组件，再发布`AgentCreated`，由WorkspacePlugin按请求ID绑定
其他组件。每个实例同时挂载`AgentIdentity`，保存完整资源ID，例如`agent:demo/coder:latest`。
Memory、Inference和Tool组件。

`AgentTokenUsage`累计普通Assistant响应的输入、输出和缓存命中Token，并维护累计缓存命中率。该状态在
Agent创建时由MemoryPlugin从历史Assistant行恢复；上下文压缩推理不进入这份对话统计。

`AgentMessage.agent`始终是已解析的Entity。User和Assistant进入长期`messages`，Tool进入当前轮
`tool_context`；三种消息都通过事件请求MemoryPlugin写历史。工具批次状态由ToolPlugin维护，最后一个响应
到达后通过`ToolTurnCompleted`通知AgentPlugin发送下一次推理请求。

每次推理请求都根据`AgentDynamicVisibility`重新构造工具定义，定义名保持完整`ResourceId`。
API合法工具名的双向转换只由InferencePlugin在当前请求内完成，不保存为Agent组件。

Memory写入失败继续使用`AgentMemoryWriteFailed`；其他消息处理或工具定义准备错误统一发送
`AgentFailure { kind: Agent }`，不再静默丢弃，也不伪造成Assistant或Tool消息。

AgentPlugin公开`AgentContextCompactRequest`作为压缩机制入口，但不主动发送该事件，也不决定触发阈值。
请求显式指定原样保留的末尾消息数量；压缩期间Agent进入工作状态。摘要成功后，AgentPlugin用一条带
`compacted-summary`标记的User检查点替换较早消息，并通过`rewrite_messages`更新实时记忆；历史消息不修改。
