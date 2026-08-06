# AgentPlugin

`AgentPlugin`只拥有Agent实例的Workspace关联、上下文、默认/动态可见性和轮次状态。它消费
`AgentCreateRequest`创建Agent自身组件，再发布`AgentCreated`，由WorkspacePlugin按请求ID绑定
Memory、Inference和Tool组件。

User和Assistant消息进入`AgentContext`前先通过MemoryPlugin追加历史；Tool响应只进入上下文和实时
表，不进入历史。一个工具批次的全部调用ID保存在`AgentStatus`，只有最后一个响应到达后才发送
下一次推理请求。

每次推理请求都根据`AgentDynamicVisibility`重新构造工具定义。工具名称到`ResourceRef`的映射只在
当前请求处理期间存在，不保存为Agent组件，也不承担权限检查。
