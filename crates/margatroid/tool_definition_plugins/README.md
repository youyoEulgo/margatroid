# Tool Definition Plugins

本目录保存具体资源工具Plugin。每个Plugin接收自己的资源注册事件，构造`ToolTemplate`并通过
ToolPlugin注册Agent专属映射；运行时接收ToolPlugin路由后的`ToolCallRequest`，执行具体资源并发送
`ToolCallResponse`。ToolPlugin统一拥有Pending调用、响应整理和`AgentMessage::Tool`构造。

当前定义包括SkillPlugin、WorkflowPlugin和LuaPlugin。LuaPlugin使用内嵌Lua 5.4异步执行开发者
安装的可信Tool资源，并在`lua_plugin/examples/tools/`保存可追踪的完整工具示例。
