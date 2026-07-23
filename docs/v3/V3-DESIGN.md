# Margatroid V3 重构设计

2026-07-10 开始，2026-07-22 正式入口切换到 ECS，状态：基础设施重构中。

## 核心问题诊断

V1/V2 的根本问题：**试图用 LLM 做流程控制**。强制模型每次回复必须调用工具（delegate/finish/bash），导致模型频繁不按要求执行，需要不断塞约束消息重试。

## V3 核心转变

**从 LLM 驱动协作 → 规则驱动协作 + LLM 执行节点**

LLM 回归它擅长的（理解、生成、判断），程序做它擅长的（流程控制、状态管理、并发调度）。

## 已明确的架构方向

API 规范已拆分为：

- [V3-INFRASTRUCTURE-API.md](V3-INFRASTRUCTURE-API.md)：可复用 ECS 与基础设施公开 API。
- [V3-BUSINESS-PLUGIN-API.md](V3-BUSINESS-PLUGIN-API.md)：Margatroid 业务 Plugin 契约。
- [V3-PROTOCOL-API.md](V3-PROTOCOL-API.md)：CLI 与 daemon 共享的产品协议。
- [V3-ROADMAP.md](V3-ROADMAP.md)：从当前状态到 v0.1 独立发布的执行阶段与验收门槛。

### 1. 守护进程（Daemon）

- 一个 bin：`margatroidd`（守护进程），两个前端：`margatroid` CLI + Web
- 守护进程常驻，管理多个 workspace 生命周期
- CLI/Web 通过 HTTP API 与守护进程通信
- CLI 交互参考 Docker 与 Docker Compose：`margatroid compose up -d`、`margatroid ps`
- daemon 持有权威资源库、运行状态和持久化数据，CLI 不直接修改 daemon 状态

当前 `margatroidd` 已直接创建 V3 `App` 并安装 `MargatroidDaemonPlugins`；
`margatroid` 已改为纯 HTTP 客户端，不再链接旧 server 或 runtime。旧实现目录已从
workspace 编译图排除，仅作为迁移参考，见 [legacy/README.md](../../legacy/README.md)。

#### 1.1 CLI 是本地工具链，不只是 HTTP 转发器

`margatroid` CLI 的职责包括：

- 解析命令和展示结果
- 读取并解析用户指定的 `compose.toml`
- 解析相对路径，收集 Soul、Skill、Workflow 等本地资源
- 执行本地语法与 schema 预检，生成稳定 `WorkspaceBundle`
- 将资源包上传给 daemon，并通过 HTTP 管理 workspace 和资源库
- 为 Agent、Skill、Provider 等资源提供 list、inspect、add、remove 命令

CLI 不负责业务状态机、运行时调度、权限裁决或持久化真相。daemon 必须对任何客户端
提交的 `WorkspaceBundle` 再做一次权威校验，不能信任 CLI 已经检查过的数据。

```text
compose.toml + local resources
→ margatroid CLI: parse / resolve / preflight / bundle
→ WorkspaceBundle + ResourceManifest
→ margatroidd: authoritative validation / persistence
→ WorkspacePlugin: create ECS runtime state
```

这种边界允许 CLI 与 daemon 将来运行在不同机器上。compose 中不得依赖 daemon
直接读取 CLI 机器上的任意绝对路径；本地文件必须进入上传资源包，或者引用 daemon
中已经安装且具有稳定 ID 的资源。Provider secret 不进入资源包，只引用 daemon 侧配置。

#### 1.2 Docker 风格命令语义

```text
margatroid compose up [-d] [-f compose.toml] [-p project]
margatroid compose stop | start | restart | down
margatroid compose ps
margatroid compose logs [-f]
margatroid compose config

margatroid ps
margatroid inspect <workspace>
margatroid logs [-f]
margatroid prompt <workspace> <text>
margatroid chat <workspace>
margatroid attach <workspace>
margatroid exec [-it] <workspace> <command...>
margatroid request inspect | watch | cancel <request-id>

margatroid agent ls | inspect | add | remove
margatroid skill ls | inspect | add | remove
margatroid provider ls | inspect | add | remove
```

- `compose up` 默认 attach，持续显示业务事件、Agent 输出和 workflow 结果。
- `compose up -d/--detach` 完成启动后返回；detach 不是静默，仍输出 workspace 名称和 ID。
- `compose logs -f` 查看并跟随 workspace 业务输出，不以 tracing 日志代替业务协议。
- 顶层 `logs -f` 查看 daemon 运维诊断日志。
- `stop` 保留 workspace 状态；`start` 恢复；`down` 删除运行实例，但不删除共享资源库。
- `compose ps` 查看当前 compose 项目，顶层 `ps` 查看 daemon 中的全部 workspace。
- `compose config` 输出 CLI 解析后的规范化配置，不创建 workspace。
- `-p/--project-name` 显式指定项目名；未指定时依次使用 compose 名称和目录名。
- `prompt` 提交工作，`request` 命令组查询、跟随或取消一次请求；它们不属于 compose 生命周期。
- `chat/attach` 建立 workspace 的交互式会话；`exec` 请求在 workspace 策略允许范围内
  执行命令，`-i` 转发 stdin，`-t` 分配 PTY。这些命令依赖终端、双向 transport、鉴权
  和 sandbox，不得退化为 CLI 直接访问 daemon 本地进程。

运维日志同样复用 daemon 的 HTTP 服务，不另开第二个日志端口：

```text
daemon tracing
→ LogPlugin bounded Stream Layer
→ ServerPlugin 鉴权日志路由
→ HttpServerPlugin SSE/WebSocket
→ margatroid logs -f
```

日志流只用于诊断。任务进度、LLM 结果和可执行错误仍通过业务 ECS Event
与产品 API 传输，不将日志当成 CLI 业务协议。

### 2. ECS + Plugin 架构（定制轻量版，借鉴 Bevy）

Bevy 值得借鉴的核心是通过 Plugin 组合功能，而不是机械地让 ECS 内核也伪装成 Plugin。
V3 中 `core_plugin` crate 是编译期内核，`App::new()` 直接创建 ECS；所有可选能力才是运行时 Plugin。

这套可独立使用和发布的 ECS 与基础设施体系暂定名为 **mecs**。
设计目标不只是可插拔，还包括开发者友好、配置简单和默认开箱即用：

- 常见场景只需 `add_plugins(...)`。
- 进阶需求使用 builder，不迫使普通用户理解内部 worker、channel 和全局状态。
- 保持 tracing、Axum 等 Rust 生态的原生使用习惯，不为形式一致重造 API。

```rust
App::new()
    .add_plugins(MargatroidDaemonPlugins::default())
    .run();
```

#### 2.1 core_plugin（ECS 内核）

core 遵循 KISS：只提供建立和运行同步 ECS 所必需的能力。

- `App` — 持有 `World` 和按阶段组织的 `Schedule`
- `World` — Entity/Component/Resource/Event 的存储容器
- `Entity` — 带 generation 的轻量整数 ID
- `Component` / `Bundle` — Entity 上的纯数据
- `Resource` — World 级类型单例
- `Query` / `Res` — 类型化访问 API
- `System` / `Schedule` — 同步逻辑和确定性排序
- `Event` / `EventReader` — System 与 Plugin 的类型化通信
- `Plugin` / `PluginGroup` — 可选功能的安装协议
- `App::tick()` — 执行一次同步 ECS 帧

core 明确不负责：

- Tokio 或其他异步 runtime
- task spawn、任务队列、完成通道
- timeout、cancel、并发上限
- 阻塞运行循环、线程唤醒和进程关闭控制
- Input/Prepare/Execute/Finalize 等 Margatroid 领域阶段
- LLM、HTTP、sandbox、skill 等领域逻辑
- workflow、agent、memory 等业务状态机

`CorePlugin` 类型删除。一个不注册任何能力的 no-op Plugin 只增加概念，不增加可插拔性。

阻塞运行循环由 `AppRuntimePlugin` 提供，异步执行由 `AsyncRuntimePlugin` 提供。
core 只保留 `Startup / First / Update / Last` 四个通用阶段，不知道业务阶段或基础设施用途。

#### 2.2 mecs 官方 Plugin 与 Margatroid 默认组合

```
基础设施（mecs）
├── LogPlugin           ← tracing console / file / bounded stream
├── AppRuntimePlugin    ← run / wake / shutdown
├── SignalPlugin        ← 任意进程信号 → typed Event
├── TerminalInputPlugin ← 本地按键 / paste / resize → typed Event
├── PtyPlugin           ← 交互式子进程与双向 PTY 字节流（规划）
├── AsyncRuntimePlugin  ← 可选异步任务执行基础设施
├── HttpServerPlugin    ← Axum / HTTP / SSE / WebSocket 生命周期
└── ExternalEventPlugin ← 外部线程安全注入 ECS Event

Margatroid 业务
├── LLMPlugin           ← Provider + Chat streaming
├── SandboxPlugin       ← 沙箱执行
├── SkillPlugin         ← Skill 加载/卸载/分发
├── WorkflowPlugin       ← Workflow DAG 执行器
├── EventBusPlugin      ← SSE 事件流
├── ServerPlugin        ← Margatroid HTTP API 与日志流路由
├── DaemonLifecyclePlugin ← readiness 与 Starting/Ready/Draining/Stopped
└── ConfigPlugin        ← daemon 运行配置加载与重载
```

`MargatroidDaemonPlugins` 由 `margatroid_defaults` crate 提供。该 crate 只负责默认
Plugin 组合，不负责创建 App、解析进程参数或启动 daemon。

mecs 的官方 Plugin 目录按“通用 ECS 工具包是否应提供该能力”设计，不按 Margatroid
当前是否使用设计。`TerminalInputPlugin` 和 `PtyPlugin` 不进入 daemon 默认组合，仍然是
mecs 的正式规划能力。可选表示使用者可以不安装，不表示 mecs 不提供。

#### 2.3 外部输入进入 ECS

HTTP handler 运行在 HTTP worker，不能直接修改 World。当前
`ExternalEventPlugin` 已打通通用基础设施部分：

```text
external thread / handler
→ ExternalEventSender<E>::try_send
→ bounded channel + AppControl::wake
→ Stage::First
→ ECS Event
```

具体 prompt HTTP 路由将在 workflow/workspace Plugin 提供实际消费者后实现。提交类请求
届时不在 handler 中等待 ECS 完成，而是返回 `202 + request_id`；在没有消费者时不得
注册路由或返回 `202`。这避免在 ECS Event 中携带 oneshot sender，也不把 HTTP
连接寿命与业务 System 的完成时间强绑定。

#### 2.3.1 进程信号、终端输入与 PTY

三者是不同的基础设施边界：

```text
SignalPlugin
  OS process signal → ProcessSignalReceived

TerminalInputPlugin
  local terminal/stdin → Key / Paste / Mouse / Resize Event

PtyPlugin
  child pseudo-terminal ↔ binary input/output Event
```

`SignalPlugin` 不直接关闭 App。Margatroid 的 `DaemonLifecyclePlugin` 消费
`Interrupt/Terminate` 后调用 `AppControl::shutdown()`；其他 mecs 应用可以把相同信号
映射为暂停、保存或忽略。

未来交互式产品链路分为两种：

```text
margatroid chat / attach
local TerminalInputPlugin → HTTP/WebSocket → workspace conversation

margatroid exec -it
local TerminalInputPlugin → bidirectional transport → remote PtyPlugin → sandboxed process
```

终端 Plugin 只处理本地终端协议，PTY Plugin 只处理子进程伪终端。workspace 对话、远程
鉴权、命令权限与 sandbox 策略仍属于 Margatroid 产品层。

#### 2.4 每个 Plugin 是一个功能边界

- 换 LLM provider → 换 LLMPlugin，其余不动
- 不用沙箱 → 去掉 SandboxPlugin，换 ProcessPlugin
- 第三方可写 Plugin 注册进 App，不碰 Margatroid 源码
- 纯同步 Plugin 测试时只需要 `App::new()`
- 异步 Plugin 测试时显式组合 `AsyncRuntimePlugin`

#### 2.5 compose.toml 编译为 WorkspaceBundle

compose 是面向用户的声明式项目文件，不是 ECS Plugin 源码。CLI 使用独立的纯数据解析库
将其编译为稳定 `WorkspaceBundle`；daemon 校验并持久化后，由 `WorkspacePlugin` 应用到 ECS：

- 每个 Agent 定义 → 创建 Entity 并挂载 Soul、SkillSet、ProviderConfig 等 Component
- 每个 Workflow 定义 → 注册到 `WorkflowRegistry` Resource
- manager 引用 → 解析为 workspace 内的普通 Agent ID
- 本地 Soul、Skill、Workflow 文件 → 进入带内容哈希的 `ResourceManifest`
- 已安装资源 → 通过稳定资源 ID 引用 daemon 的资源库

compose 解析器和 bundle 类型不依赖 ECS、HTTP、CLI 或 daemon 实现。CLI 可做快速预检，
但 daemon 的校验结果始终是最终结果。

#### 2.6 ECS 核心概念映射

- Entity = Agent 实例（只是 ID）
- Component = 纯数据（SoulPrompt, SkillSet, TaskContext, ProviderConfig, Memory 等）
- System = 纯逻辑，通过 label + ordering 约束（`after(x)`, `in_set(Stage)`）
- Plugin = 功能组合单元，打包一组 System + Component
- Resource = World 级全局单例（EventBus, TaskDAG, Sandbox 等）

### 3. Agent 并发（树/DAG）

- TaskChain 从单向链表变为树/DAG
- Agent A 可以同时 delegate 给 B 和 C，并行执行后汇合结果
- System 调度支持 concurrency（工作流 skill 内的分支天然可并行）

### 4. 用户脱离成员身份 + Manager 入口

- 用户不在委托链上作为节点
- 用户是工作流的触发者和中断仲裁者，通过 CLI/Web 发指令
- compose.toml 声明 `manager` 字段，指定哪个 agent 接收用户消息
- manager 是普通 agent，无特殊身份，不硬编码特权工具
- 缺 `manager` 字段 → 启动时直接报错退出，不做 fallback

### 5. Agent 功能统一 + 人格隔离

- 废除 Identity 枚举（Manager/Member/User）
- 所有 agent 底层能力完全相同：接到任务 → 执行 → 出结果
- 差异在于：SOUL.md（人格）+ SkillSet（技能组合）
- 动机：同一人格写代码+同一人格审查 = 盲区叠加，不同人格才能互相发现问题
- 同一个 Entity archetype，挂不同 Component 值即可区分
- 需要一个 coordinator agent（普通 agent，人格/技能偏向路由），不是特殊身份，用户也可绕过它直接指定目标

### 6. 统一的 Skill 格式

两类 skill 使用相同的外表格式（TOML frontmatter + Markdown 正文），LLM 看到的列表完全统一。区分靠程序解析正文结构。

**共同特征：**
- 后缀 `.md`
- frontmatter 包含 `name`, `description`, `allowed_tools` 等标准字段
- LLM 视角：所有 skill 都是"可以加载的能力"，选哪个都一样
- `load_skill("xxx")` 被调用后，程序先判定类型，再分派处理

**分类判定逻辑：**

```rust
fn classify(skill_body: &str) -> SkillKind {
    if skill_body.contains("[[steps]]") {
        SkillKind::Workflow
    } else {
        SkillKind::Member
    }
}
```

#### 6.1 Member Skill

Frontmatter 之后是面向 LLM 的自然语言 Markdown 提示词模板。

**Frontmatter 字段：**

| 字段 | 类型 | 说明 |
|---|---|---|
| `name` | string | skill 标识符，也是委托工具名 `delegate_to_{name}` |
| `description` | string | 显示给 LLM 的简介 |
| `allowed_tools` | string[] | 此成员可用工具白名单，留空则继承默认工具集 |
| `preload` | bool | 是否 workspace 创建时自动预加载 |

**Markdown 正文约定：**

- 能力边界：该成员擅长和不擅长的场景
- 交流守则：如何描述任务、期望的输出格式
- 交付格式：任务结果的期望结构
- 可选 `{variable}` 占位符，运行时替换

**加载后效果：**

1. 注入一段 `{SOUL.md 片段}` 到 system prompt，描述该成员
2. 注入专用委托工具 `delegate_to_{name}`，参数为结构化 `task_spec`
3. 工具被调用后，程序接管执行

**卸载：** 摘掉对应 Component → 工具消失，提示词消失

**示例：**

```markdown
+++
name = "coder"
description = "Rust 实现者，从 API 规格出发写代码"
allowed_tools = ["bash", "delegate", "recall"]
preload = true
+++

# coder — 协作指南

## 信任边界
- 擅长：Rust/Go 实现，调试，性能优化
- 不擅长：架构设计，需求分析，安全审计

## 交付规范
委托时请提供 {task_spec}：
- 目标文件路径
- 接口/类型定义
- 期望的行为描述（输入→输出）

## 输出格式
生成代码后附带简短说明，如需要进一步委托请明确说明需要谁配合。
```

#### 6.2 Workflow Skill

Frontmatter 之后是纯 TOML 的 `[[steps]]` 块——对 LLM 来说只是看起来像数据的 Markdown 代码块，对程序来说是确定性执行图。

**Frontmatter 字段（与 member skill 相同基础字段）：**

| 字段 | 类型 | 说明 |
|---|---|---|
| `name` | string | skill 标识符 |
| `description` | string | 显示给 LLM 的简介，描述此工作流的用途 |
| `allowed_tools` | string[] | 工作流中不需要 LLM 自由选择工具，通常为空 |
| `preload` | bool | 通常 false，由 LLM 按需加载 |

**Steps 定义：**

| Step 类型 | 字段 | 说明 |
|---|---|---|
| `delegate` | `target`, `prompt_template`, `depends_on` | 委托给某个 member，等待结果 |
| `condition` | `condition`, `then`, `else` | 基于前置结果的分支判断 |
| `parallel` | `steps`, `depends_on` | 并行执行一组子步骤 |
| `return` | `value` | 将结果返回给上游（coordinator） |

**Steps 通用字段：**
- `id` — 可选，步骤标识符，用于 condition 引用和 prompt_template 中的变量名
- `depends_on` — 可选，依赖的 step index 列表，不填则默认依赖前一步

**变量系统：**
- 每个 step 的输出以 `id` 为变量名（无 id 则以 step index 命名）
- `{coder.output}` 引用 coder step 的完整输出
- `{requirement}` 等用户输入变量由 coordinator 传入

**加载后效果：**
1. LLM 视角：看到 skill 列表中有此工作流
2. LLM 调用 `load_skill("code_review")` 加载
3. 程序检测到 `[[steps]]` → 判定为 workflow
4. 程序直接接管，按 steps DAG 确定性执行
5. 每个 delegate step 内部仍然调用该 member 的 LLM，但流程本身不受 LLM 控制

**示例：**

```markdown
+++
name = "code_review"
description = "编码→审查→修复 三阶段工作流，适用于所有新功能的开发"
allowed_tools = []
preload = false
+++

[[steps]]
id = "impl"
type = "delegate"
target = "coder"
prompt_template = """
在 {path} 实现以下功能：

{requirement}
"""

[[steps]]
id = "review"
type = "delegate"
target = "reviewer"
prompt_template = """
请审查以下代码实现，重点关注安全性、性能、代码风格和测试覆盖。

{impl.output}
"""
depends_on = [0]

[[steps]]
type = "condition"
condition = "{review.output} contains markdown header '## 需要修改'"
then = "fix"
else = "return"

[[steps]]
id = "fix"
type = "delegate"
target = "coder"
prompt_template = """
根据以下审查意见修改代码，逐一处理所有问题：

审查意见：
{review.output}

原始实现：
{impl.output}
"""
depends_on = [1]

[[steps]]
id = "recheck"
type = "delegate"
target = "reviewer"
prompt_template = """
请确认以下修改已解决所有审查问题，且未引入新问题。

修改后代码：
{fix.output}
"""
depends_on = [3]
```

### 7. Skill 加载与卸载流程

```
LLM: 调用 load_skill("coder")
  ↓
SkillRegistry.lookup("coder")
  ↓
  ├── MemberSkill:
  │     1. 挂载 MemberComponent { name, prompt_fragment }
  │     2. 注册工具 delegate_to_coder(tool_spec)
  │     3. 工具参数结构由 skill frontmatter 定义
  ↓
  ├── WorkflowSkill:
  │     1. 程序解析 [[steps]] 为执行 DAG
  │     2. 开始按步执行，每个 delegate step 调用对应 member
  │     3. 执行完成后 control 归还给触发 workfow 的 agent
  ↓
LLM: 调用 unload_skill("coder")
  ↓
  摘除对应 Component + 移除工具注册
```

### 8. 约束机制总结

不是每轮对话都约束，只在关键分叉点注入强约束：

| 节点 | 约束方式 | 约束强度 |
|---|---|---|
| 任务开始时 | system prompt 注入：可用 skill 列表 + 选择规则 | 弱（建议性） |
| 需要委托他人时 | 只有 member skill 已加载的 member 可见，工具只列已加载的 | 中（选择性暴露） |
| 工作流 skill 被加载时 | 程序完全接管，LLM 只能在 prompt_template 内发挥 | 强（确定性执行） |
| 被委托执行任务时 | prompt_template + task_spec 约束上下文 | 中（结构化输入） |

### 9. 记忆隔离

基于人格隔离原则，记忆不应全局共享。每个 agent 的记忆是私有的。

#### 设计原则

- **personal_memory 完全隔离** — 每个 agent 独立 `personal.db`，无法读其他 agent 的回忆
- **conversation_messages 隔离** — agent 与 LLM 的对话历史是私有的
- **worklog + delegations 共享** — 团队需要知道"谁做了什么的产出"，这是协作基础
- **manager 可读 worklog，不能读 personal** — 与现实团队一致

#### 存储结构

```
~/.margatroid/workspaces/{name}/
├── agents/
│   ├── coder/
│   │   ├── personal.db
│   │   └── conversation.db
│   └── reviewer/
│       ├── personal.db
│       └── conversation.db
├── team.db           ← worklog + delegations（团队共享）
└── compose.toml
```

#### ECS 映射

- 每个 agent Entity 挂 `MemoryComponent`（持有自己的 `personal.db` + `conversation.db` 连接）
- `TeamMemory` 作为 World Resource（持有 `team.db` 连接）
- `MemorySystem` 读写 `MemoryComponent`，`WorklogSystem` 读写 `TeamMemory`
