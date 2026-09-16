# ToolPlugin

ToolPlugin保存通用Loader和静态工具的`ToolTemplate`，并把定义检查与领域工具调用路由给具体工具Plugin。

```text
ToolDefinitionRequest -> ToolDefinitionRoute -> ToolDefinitionResult
ToolCallRequest -> ToolCallEvent -> AgentMessage::Tool
```

`ToolCall`直接保存完整`ResourceId`。ToolPlugin只检查对应模板是否注册，不读取Skill或Workflow文件，
不解析工具参数，不执行handler，也不检查Agent可见性。

`AgentToolEnvironment`只保存项目根和镜像根，供具体资源Plugin按项目、镜像、主目录顺序查找资源。

## 工具执行不在 daemon 进程内

Lua 工具的执行已经搬到一个独立的可执行文件 `tool_runner`（本 crate 的 bin 目标）：

```text
daemon（tool_plugin） → spawn tool_runner → 请求走 stdin，结果走 stdout
```

- 请求是一个 JSON（字段与 `LuaToolRunRequest` 对齐，时间以毫秒表示），由 `spawn_tool_runner` 序列化后写进 runner 的 stdin
- runner 把工具的返回值原样写 stdout；出错时以非零退出并把 `{"kind", "message"}` 写到 stderr，
  由 `spawn_tool_runner` 还原成同样的 `ToolError`
- runner 的路径默认取 daemon 可执行文件同目录，可用 `MARGATROID_TOOL_RUNNER` 覆盖
- runner 起不来、流失败或超时记为 `RunnerFailed`，与"工具执行失败"分开

这样做的原因：沙箱只能约束进程，工具执行必须先有一个进程边界，才谈得上把它关进沙箱（下一步）。

