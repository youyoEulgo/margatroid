# WorkflowPlugin

该Plugin的`workflow:* -> tool:builtin/workflow-loader:latest`设计废止，实现阶段删除此crate及其组合。

Workflow统一表示为`mcl:<scope>/<name>:<tag>`资源，入口文件为`main.lua`，由MclPlugin负责解析、
挂载、运行和卸载。Workflow不是Builtin Tool，不进入BuiltinToolPlugin注册路由，也不通过
ToolCallRequest旁路执行。
