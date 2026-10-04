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
- runner 起不来、流失败或超时记为 `RunnerFailed`：这是环境问题，会结束当前轮；
  工具自身返回的业务失败则留给模型重试

这样做的原因：沙箱只能约束进程，工具执行必须先有一个进程边界，才谈得上把它关进沙箱。

## 沙箱

沙箱策略来自镜像中的具名 `sandbox` 资源，由 `base.lua` 通过 MCL `IMPORT` 引入，
再用 `BIND`/`LOAD` 与 `sandbox_use` 选择。每次工具调用都变成：

```text
landstrip run -p <镜像策略文件> -- tool_runner
```

后端定位顺序：`MARGATROID_SANDBOX_BACKEND` → `PATH` → daemon 可执行文件同目录。
策略故障会 fail-closed，不会绕过沙箱执行。

工具进程继承宿主环境的正常 CLI 部分（`HOME`、`PATH`、locale、代理等），
只剔除名称含 `KEY`、`PASSWORD`、`SECRET`、`TOKEN` 的凭据形变量，并覆盖
`NO_COLOR`、`TERM`、`PAGER` 与 `GIT_PAGER` 以稳定输出。

## 工具里能用什么

宿主在工具 VM 里只注入一个入口函数：

```lua
local api = margatroid()   -- { version, arguments, context, json, shell }
```

- `arguments`：本次调用的入参（已按 input.schema.json 校验过）
- `context`：本次轮次与工具包只读元数据（agent / turn / resource / 各层根目录）
- `json`：`encode` / `decode`
- `shell(command, options?)`：经 runner 执行命令，返回
  `{ exit_code, stdout, truncated, terminal }`；pty 不可用时自动退化为 pipe
- 工具不应使用标准库 `io.popen` 或 `os.execute` 执行命令，因为它们没有统一的
  pty、进程组回收、超时与输出限额；命令统一走 `margatroid().shell`
- shell 资源只提供解释器脚本，不是模型可调用的工具；模型看到的是 Lua 工具资源

