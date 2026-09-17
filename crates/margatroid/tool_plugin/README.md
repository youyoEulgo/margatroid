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

## 沙箱（临时脚手架）

工具执行外面可以套一层沙箱，开关**不来自配置**，而是硬编码一个路径：

    /tmp/margatroid-sandbox-policy.json

- 这个文件**存在** ⇒ 每次工具调用都变成 `landstrip run -p <该文件> -- tool_runner`，
  并且 daemon 启动时会先跑 `landstrip doctor` 与 `landstrip policy validate -p <该文件>`，
  任一失败即拒绝启动（fail-closed）
- 这个文件**不存在** ⇒ 行为与没有沙箱时完全一致
- 后端定位顺序：`MARGATROID_SANDBOX_BACKEND` → `PATH` → daemon 可执行文件同目录
- 两条分支都会 `env_clear()` 后只传 `PATH`，避免把 daemon 的环境（含 token）带给被沙箱化的进程

这是刻意的一次性形态：最终的沙箱策略是镜像里的具名资源，由 `base.lua` 用 `IMPORT` 引入、
再用 MCL 指令启用（见设计文档 §3.1、§5.6）。到那时这个硬编码路径会连同它的校验一起删掉。

