# AgentPlugin

`AgentPlugin`只拥有Agent实例的Workspace关联、上下文、默认/动态可见性和轮次状态。它消费
`AgentCreateRequest`创建Agent自身组件，再发布`AgentCreated`，由WorkspacePlugin按请求ID绑定
其他组件。每个实例同时挂载`AgentIdentity`，保存Workspace生成的稳定Agent ID，例如`demo.coder0`。
Memory、Inference和Tool组件。

`AgentMessage.agent`始终是已解析的Entity。User和Assistant进入长期`messages`，Tool进入当前轮
`tool_context`；三种消息都通过事件请求MemoryPlugin写历史。一个工具批次的全部调用ID保存在`AgentStatus`，只有最后一个响应到达后才发送
下一次推理请求。

每次推理请求都根据`AgentDynamicVisibility`重新构造工具定义。工具名称到`ResourceRef`的映射只在
当前请求处理期间存在，不保存为Agent组件，也不承担权限检查。

Memory写入失败继续使用`AgentMemoryWriteFailed`；其他消息处理或工具定义准备错误统一发送
`AgentFailure { kind: Agent }`，不再静默丢弃，也不伪造成Assistant或Tool消息。
