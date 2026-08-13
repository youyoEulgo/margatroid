# Tool Definition Plugins

本目录保存具体资源工具Plugin。每个Plugin注册一个通用Loader模板，接收`ToolDefinitionRoute`和
`ToolCallEvent`，自行检查、读取并执行对应资源，最后直接发送`ToolDefinitionResult`或
`AgentMessage::Tool`。ToolPlugin不持有这些Plugin的执行器。
