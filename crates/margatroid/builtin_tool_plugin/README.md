# BuiltinToolPlugin

BuiltinToolPlugin组合Margatroid内建的Skill、Workflow、Lua和Shell执行器，并把Workspace提交的可见
资源注册请求路由到对应执行器。

```text
skill:*    -> tool:builtin/skill-loader:latest
workflow:* -> tool:builtin/workflow-loader:latest
tool:*     -> tool:builtin/lua-runtime:latest
shell:*    -> tool:builtin/shell:latest
```

右侧内建工具只存在于ToolMap内部，对LLM隐藏；LLM看到并调用的是左侧资源生成的ToolSpec。
ShellPlugin的`examples/`包含可直接安装到主目录的基础`shell:local/sh:latest`资源；它把
`command`作为单个参数传给`main.sh`，并在Agent项目目录中执行。
完整接口与边界见`DESIGN.md`。
