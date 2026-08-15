# Tool Definition Plugins

本目录保存具体资源工具Plugin。每个Plugin接收自己的资源注册事件，构造`ToolTemplate`并通过
ToolPlugin注册Agent专属映射；运行时接收ToolPlugin路由后的`ToolCallRequest`，执行具体资源并发送
`ToolCallResponse`。ToolPlugin统一拥有Pending调用、响应整理和`AgentMessage::Tool`构造。

当前定义包括SkillPlugin、WorkflowPlugin、LuaPlugin和ShellPlugin。它们由上层BuiltinToolPlugin
统一安装和路由，隐藏`tool:builtin/*`执行器，只把资源ToolSpec暴露给LLM。LuaPlugin使用内嵌
Lua 5.4执行可信Tool资源；ShellPlugin通过异步子进程执行可信Shell资源。
