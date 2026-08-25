# Margatroid V3 架构设计

状态：以 MECS 事件驱动 ECS 为底座，按领域 Plugin 组织；当前代码与各 crate 的 `DESIGN.md` 为权威文件。

本文只描述产品结构、稳定概念和主要运行路径。具体公开类型、函数与内部逻辑以对应 crate 的 `DESIGN.md` 和 `README.md` 为准。

## 1. 产品目标

- AgentImage 类似容器镜像，是 Agent 可启动的静态资源集合。
- AgentInstance 是运行中的 Agent，只在启动时读取镜像。
- Workspace 由 Workspace 文件编排的一组 AgentInstance。
- Memory 默认按项目和 Agent 自动分配 SQLite 文件。
- MCL（Model Context Language）通过 Base Lua 驱动 Agent 的消息循环、上下文管理、推理和工具调用。

当前为本地单机产品：CLI 与 daemon 共享文件系统，CLI 通过 WebSocket 连接 daemon。

## 2. 分层

```text
apps/
├── cli                 短生命周期本地控制端
└── daemon              产品组合根

crates/mecs/
├── core_plugin         同步 ECS
├── app_runtime_plugin  事件驱动 Runtime
├── async_runtime_plugin 异步任务与 Async System
├── log_plugin          结构化日志与 TracingStream
├── server_plugin       WebSocket Server 与连接注册
├── signal_plugin       信号处理
└── closure_plugin      通用闭包 System

crates/margatroid/
├── types               共享纯类型与静态契约
├── protocol            CLI/daemon 共享 DTO 与消息协议
├── compose             Workspace 文件编译器
├── resource_id_plugin  ResourceId 身份基础
├── agent_image_loader_plugin  AgentImage 加载与布局校验
├── config_plugin       全局配置 Resource
├── connection_plugin   WebSocket 连接元数据注册
├── dto_plugin          入站/出站 DTO 转换与 WebSocket 消息路由
├── lua_runtime_plugin  Lua VM 创建、调度、宿主函数与邮箱
├── mcl_plugin          MCL 命令解析、Block、Effect、推理/工具协调
├── inference_plugin    Provider 路由、HTTP 流式推理
├── tool_plugin         工具调用路由与 skill/hook/lua/shell 执行器
├── memory_plugin       SQLite 历史与实时上下文存储
├── agent_plugin        Agent 组件、创建、消息投递、生命周期
└── workspace_plugin    Workspace 运行对象与 Agent 编排
```

## 3. Runtime 模型

- MECS Core 只提供 `App`、`World`、`Entity`、`Component`、`Resource`、`Event`、`Schedule` 和 `System`。
- System 同步接收 `&mut World`，只读取本帧事件并调用 handler，不展开跨帧业务逻辑。
- 异步任务通过 `AsyncRuntimePlugin` 提交，外部线程通过有界通道/事件回到主线程，不持有 `World`。
- Plugin 按依赖顺序安装，重复安装或缺少依赖时 panic。

## 4. 主要运行路径

```text
CLI workspace up
  -> 编译 margatroid-workspace.yaml 为 WorkspaceDefinition
  -> 连接 daemon WebSocket /ws
  -> workspace.start
  -> dto_plugin 解码并发送 StartWorkspace
  -> workspace_plugin 收集 AgentImage、模型、工具、记忆材料
  -> 发送 AgentCreateRequest
  -> agent_plugin 创建 Agent Entity 并启动长期 Lua VM
  -> MclPlugin 注入 mcl 环境
  -> Base Lua 执行 IMPORT / CREATE / INJECT / EMIT EFFECT
  -> Agent 初始化完成
  -> workspace.started 返回 CLI
```

## 5. AgentImage

AgentImage 是 Agent 库中的最小可启动镜像，至少包含：

```text
agent.toml      声明镜像依赖、模型、资源和默认可见性
base.lua        Base Driver，负责 MCL 消息循环
skills/         可选 Skill 目录
hooks/          可选 Hook 目录
tools/          可选 Tool 目录
shells/         可选 Shell 目录
*.md            提示词文件（如 SOUL.md、COMPACT.md）
```

`agent_image_loader_plugin` 负责读取镜像、校验布局、加载 prompt 依赖并生成 `AgentImage` Entity。

## 6. MCL 与 Base Lua

Base Lua 是每个 Agent 的 Driver。它通过注入的 `mcl(agent_id, command, binding)` 与宿主交互：

```text
IMPORT prompt:system/soul:latest AS soul
CREATE BLOCK msg (...)
CREATE REF_BLOCK req (...)
INJECT ... TO ... FROM ...
EMIT EFFECT realtime_load
EMIT EFFECT realtime_source (req)
EMIT EFFECT history_append ...
EMIT EFFECT inference (req)
EMIT EFFECT tool_call ?
EMIT EFFECT finish
```

Base Lua 消息循环：

```text
start
  -> 收到 User：写 recent/history，发 inference
  -> 收到 Assistant：写 recent/history，若 tool_calls 非空发 tool_call，否则 finish
  -> 收到 Tool：写 recent/history，删除 pending_tool，全部完成后发 inference
  -> 收到 Error：只写 history，不进入推理或实时上下文
```

## 7. Agent 生命周期与失败

- Agent 创建期间：VM 初始化完成前保持 `Creating`，创建失败会 despawn 释放 `agent_id`。
- Agent 创建成功后：业务失败只报告错误，不终止 Agent。
- 推理失败等轮次级失败由 `AgentFailure` 表示；`mcl_plugin` 将其转换为 `AgentMessage(Error)` 投递给 Agent。
- Base Lua 收到 Error 只写历史，前端通过历史渲染。
- 工具调用成功或失败都由 `tool_plugin` 生成 `AgentMessage(Tool)`。
- 推理成功生成 `AgentMessage(Assistant)`。

## 8. 资源与可见性

- ResourceId 格式：`type:scope/name:tag`，默认 tag `latest`。
- Prompt 依赖：`prompt:<scope>/<name>:<tag>`，文件名为 `<NAME 大写>.md`。
- Agent 持有 `AgentResourceMap`：资源定义、别名、可见性、默认可见性、可执行工具候选。
- `tool_plugin` 从 `AgentResourceMap` 和 MCL 可见性生成 Provider ToolSpec；skill/hook/lua/shell 都是 tool handler 的一部分。

## 9. 工具链

```text
ToolCallEvent
  -> tool_plugin handler
  -> AgentMessage(Tool)
```

`tool_plugin` 不再有独立的 `builtin_tool_plugin` 或四个 `tool_definition_plugins`；skill/hook/lua/shell 全部位于 `tool_plugin/src/handler/`。

## 10. Memory

```text
<project>/.margatroid/workspaces/<workspace>/memory/<agent>/memory.sql
```

每个 Agent 一个独立 SQLite 文件：

```text
history_messages  User / Assistant / Tool / Error
realtime_context  ordered MclMessage { Message, Option<TokenUsage> }
```

- 历史表：客户端可展示对话的唯一来源。
- 实时表：Base Lua 恢复上下文用的有序快照；上下文压缩只替换实时表。
- Error 只进入历史表，不进入实时上下文。

## 11. API 与前端

- daemon 通过 WebSocket `/ws` 接收 `connection.register`、`workspace.start`、`workspace.stop`、`agent.message`、`agent.assistant`、`agent.turn.abort`、`mcl.command` 等客户端消息。
- daemon 向 WebSocket 发送 `state.sync`、`workspace.started`、`workspace.start_failed`、`workspace.stopped`、`workspace.stop_failed`、`agent.message`、`agent.message.delta`、`agent.message.reasoning_delta`、`agent.failure`、`mcl.command_result` 和 `log`。
- 前端以 `state.sync` 为准渲染 Workspace、Agent 状态和历史；`agent.message.delta` 只做实时累积。
- 消息目标由 `config.toml` 的 `logs`、`backend_state`、`member_messages`、`streaming_member_messages` 四组配置决定。

## 12. 重要边界

- `lua_runtime_plugin` 不解析 MCL、不创建 Agent、不注册工具、不发送 WebSocket；这些能力由 `LuaEnvironmentProvider` 注入。
- `memory_plugin` 不判断 Tool 来源、不决定压缩时机。
- `inference_plugin` 不读取 AgentResourceMap，只消费请求中已给出的 ToolSpec。
- `agent_plugin` 不解析 MCL、不执行推理/工具/记忆，只负责 Agent 组件、消息投递和生命周期。
- `dto_plugin` 不产生 Agent 领域状态，只做协议转换和 WebSocket 路由。
