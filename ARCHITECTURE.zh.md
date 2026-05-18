# Margatroid 架构设计

> 项目名取自东方 Project 角色 Alice Margatroid，称号"七色的人形使"，能力是同时操控多个人偶作战。在 Margatroid 中，用户是 AI 人形使，compose 文件定义每个智能体的能力和角色，委托板是连接智能体与用户的丝线。

## 1. 设计哲学

Margatroid 的初始灵感来自 Docker Compose——用户通过声明式配置文件编排多个 AI 智能体实例，让它们协同完成复杂任务。但在设计过程中，我们发现容器编排的隐喻存在根本性的不匹配：容器之间是请求-响应的网络通信，而智能体之间的协作更像是人类团队的分工与委托。

因此我们放弃了纯工程隐喻，转而采用**人类团队协作模型**作为设计基础。一个 Workspace 就是一个项目小组，每个 AI 智能体是小组中拥有特定技能的成员，Manager 是项目经理，委托板是团队的协作基础设施。

## 2. 核心概念

### 2.1 Workspace

一个 Workspace 是一个独立的、沙箱化的协作环境，包含：

- 一组 AI 智能体实例（由 compose 文件定义）
- 一个共享的工作目录（workdir）
- 一个 AI Manager（项目经理）
- 一个委托板（Delegation Board）
- 每个智能体的独立记忆存储（SQLite）

Workspace 之间完全隔离，各自拥有独立的沙箱环境。

### 2.2 AI 智能体实例

一个智能体实例是 compose 文件中定义的一个"团队成员"，由以下要素构成：

| 要素             | 说明                                       |
| ---------------- | ------------------------------------------ |
| Provider + Model | 底层 AI 模型（如 OpenRouter 上的某个模型） |
| Skills           | 该实例具备的能力集合（函数/工具定义）      |
| System Prompt    | 定义该成员的角色身份和行为边界             |
| Profile          | 详细的能力描述文件，供其他成员查询         |
| Memory DB        | 独立的 SQLite 数据库，存储该成员的工作记忆 |

每个实例运行在 Workspace 沙箱中，只能访问 workdir 和委托板，无法直接与其他实例通信。

### 2.3 AI Manager

Manager 是 Workspace 中的特殊成员，角色等同于项目经理：

- 直接与用户对接，接收用户需求
- 将用户需求分解为可委派的结构化任务
- 通过委托板将任务分发给合适的智能体
- 处理委托过程中的异常和纠纷（如执行智能体无法完成任务、结果被驳回等）
- Manager 本身也是调用 LLM 的智能体，不是硬编码的调度器

### 2.4 委托板（Delegation Board）

委托板是 Workspace 的核心协作基础设施，采用**双区模型**：

```
offer ──→ [发布区] ──take──→ [成员处理] ──finish/delegate──→ [档案区: SQLite]
```

- **发布区**：Manager 或成员 `offer` 委托任务进入，目标成员 `take` 领取后移除
- **档案区**：SQLite，`result` 写入时自动更新 worklog + personal_memory

**纯异步委托**：所有委托都是非阻塞的——发布后立即返回 task_id，结果通过档案区自然流转。成员不等待被委托方完成。

**用户消息**：通过 `send_user_message("user", "manager", ...)` 以根委托发布到发布区。

**记忆写入**：`add_task` 时自动插入 worklog + personal_memory 行，`add_result` 时补全 summary / detail。board 直接持有 SQLite。

**重试**：成员执行失败时任务自动重新发布（detail 前缀 `[RETRY:N]`），超过 3 次放弃。

### 2.5 计划表（Schedule）

Manager 专用的阶段任务规划工具，存于 board 的 SQLite。一条阶段任务 = 一个成员被分配的顶层工作项，每个成员同时最多只有一个阶段任务在执行。阶段任务作为委托发布到 board 上，完成后 Manager accept 时归档。

| 操作 | 效果 |
|---|---|
| `schedule_add` | 添加 planned 条目（target, description, priority） |
| `schedule_list` | 列出所有 planned 条目 |
| `schedule_pop` | 弹出某成员最高优先级条目，标记 offered |
| `schedule_remove` | 删除指定条目 |
| `schedule_reorder` | 调整优先级 |

**Manager 的 push_from_schedule**：Manager 空闲时遍历 schedule，检查目标成员是否有 offered 条目。若无则 pop → offer，将 `schedule_id` 写入委托的 parameters。Manager accept 时通过 parameters 中的 `schedule_id` 精确归档，不依赖 heuristics。

阶段任务与委托的区别：
- **阶段任务**：schedule 中的顶层规划，每成员一次一个
- **委托**：board 上的具体工作项，可以来自 Manager（阶段任务）或 peer 委托（成员间互相委托）

### 2.6 沙箱（Sandbox）

Margatroid 采用 **OS 原生沙箱方案**（参考 Claude Code 的 `sandbox-runtime`），不依赖 Docker 或虚拟机，利用操作系统内核级隔离机制：

| 平台 | 隔离机制 | 依赖 |
|------|----------|------|
| Linux | Bubblewrap (`bwrap`) — 挂载命名空间 + PID 命名空间 + 网络命名空间 | 需安装 `bubblewrap` |
| macOS | `sandbox-exec` — 动态生成的 Seatbelt 配置文件 | 系统内置 |

**双重隔离模型。** 文件系统隔离和网络隔离必须同时存在，缺一不可。仅有文件隔离则 agent 可通过网络泄露数据，仅有网络隔离则 agent 可篡改系统文件。

**文件系统规则。** 写入默认全禁（allow-only），必须显式声明可写路径；读取默认全开，但可拒绝敏感区域（deny-then-allow）。只有 workdir、`/tmp` 和 Margatroid 数据目录是可写的。宿主系统的所有二进制文件（rustc、gcc、node、python 等）以只读方式可见——agent 直接继承宿主工具链，无需在沙箱内重新安装。

**网络规则。** 默认全禁（allow-only）。所有 HTTP/HTTPS 流量经 HTTP 代理过滤，其余 TCP 流量经 SOCKS5 代理过滤，按域名白名单放行。代理运行在沙箱外的宿主机上，沙箱内进程通过 Unix Domain Socket（Linux）或 localhost 端口（macOS）连接到代理。

**强制保护路径。** 以下路径在代码中硬编码为禁止写入，不受任何配置影响：`~/.ssh/`、`~/.aws/`、`.env`、`.gitconfig`、`.git/hooks/`、`.mcp.json`。

**临时性与持久化。** 沙箱环境是临时的——Workspace 停止后沙箱销毁，下次启动从零开始。只有 workdir 和 memory.db 是持久挂载的，跟随项目存活。Agent 在沙箱内可以随意安装系统包（`apt-get`、`pip install` 等），这些改动在下次启动时自动丢弃。

**每个 Workspace 分配一个独立的沙箱。** 所有智能体实例运行在同一个沙箱内，共享文件系统和网络规则。不同 Workspace 之间完全隔离。

## 3. Compose 文件规范

Compose 文件是 Workspace 的声明式定义，采用 TOML 格式（与 Docker Compose 的 YAML 传统不同，因为 Margatroid 整体使用 TOML 作为配置格式）。

### 3.1 最小示例

```toml
[workspace]
name = "my-project"
version = "0.1.0"
description = "一个示例项目组"
workdir = "./project"

[[agents]]
id = "architect"
provider = "OpenRouter"
model = "anthropic/claude-sonnet-4"
system_prompt = "你是一个软件架构师，负责设计系统架构和做技术决策。"
skills = ["design", "code-review"]
depends_on = []

[[agents]]
id = "coder"
provider = "OpenRouter"
model = "google/gemini-2.5-flash"
system_prompt = "你是一个程序员，负责根据架构设计编写代码。遇到架构问题请委托给 architect。"
skills = ["coding", "testing"]
depends_on = ["architect"]

[[agents]]
id = "reviewer"
provider = "OpenRouter"
model = "moonshotai/kimi-k2.6"
system_prompt = "你是一个代码审查员，负责审查代码质量和安全性。"
skills = ["code-review", "security-review"]
depends_on = []
```

### 3.2 字段说明

**workspace 顶层：**

| 字段        | 类型   | 必填 | 说明                                  |
| ----------- | ------ | ---- | ------------------------------------- |
| name        | string | 是   | Workspace 唯一标识                    |
| version     | string | 是   | 配置版本号                            |
| description | string | 否   | 项目描述                              |
| workdir     | string | 是   | 项目信任目录，相对于 compose 文件位置 |

**agents 列表项：**

| 字段          | 类型     | 必填 | 说明                                               |
| ------------- | -------- | ---- | -------------------------------------------------- |
| id            | string   | 是   | 成员唯一标识符                                     |
| provider      | string   | 是   | AI 服务商名称，对应 margatroid.toml 中的 provider  |
| model         | string   | 是   | 模型 ID                                            |
| system_prompt | string   | 是   | 角色定义和基本行为边界                             |
| skills        | string[] | 是   | 拥有的能力标签列表                                 |
| depends_on    | string[] | 否   | 声明式依赖，用于自文档化协作拓扑                   |
| profile       | string   | 否   | 详细能力描述文件路径（若不指定则自动使用默认模板） |
| max_tokens    | u32      | 否   | 每次请求的最大 token 数                            |
| temperature   | f32      | 否   | 模型温度参数                                       |

**注意：** `depends_on` 字段在当前版本仅用于自文档化，不强制启动顺序。协作关系通过委托板动态建立。

## 4. 团队协作模型

### 4.1 成员发现机制

每个成员通过两层信息结构认识团队：

**第一层：公共 Roster Skill**

由 compose 文件生成，注入到每个成员的可用 skill 列表中。包含所有成员的基本信息：

```
团队中有以下成员：
- architect: 软件架构师，擅长系统设计和代码审查
- coder: 程序员，擅长编码和测试
- reviewer: 代码审查员，擅长代码审查和安全审查
```

这是一个低成本的"团队成员目录"，每个成员启动时就知道团队中有谁、各自大致做什么。

**第二层：个人 Profile**

每个成员持有一份详细的 Profile 文件，记录具体的能力范围、擅长的技术栈、工作偏好等。当成员 A 通过 roster 判断某任务可能适合成员 B 时，通过委托板查询 B 的完整 Profile，确认后再发起委托。

这种两层设计避免了将所有成员的详细能力塞入每个 agent 的上下文窗口。Roster 充当 Bloom filter——快速过滤"谁可能能帮忙"——Profile 在确认候选后才加载。

### 4.2 委托流程

```
1. 成员 A 在执行任务时发现需要 B 的能力
2. A 通过 roster skill 识别 B 可能是合适人选
3. A 调用 delegate 工具，指定 target、task_summary、task_detail，附带自己当前产出的 work_summary/work_detail
4. board 记录 A 的阶段性产出（done=false），发布新委托到发布区
5. B 领取任务，执行，完成后调用 finish 工具
6. 结果写入档案（SQLite）
```

### 4.3 工具驱动的委托循环

成员通过 `chat()` 中的 tool-call 循环与委托板交互：

- `finish` —— 完成当前委托，产出最终结果（done=true）
- `delegate` —— 记录阶段性产出（done=false），发布子委托，退出循环
- `bash` / `recall` / `schedule_*` —— 执行后继续循环
- 裸文本回复 —— 追加循环约束，强制 LLM 调用 finish 或 delegate

### 4.4 Manager 任务分解

Manager 接收用户需求后：

1. 查询委托板上的 roster 了解团队能力现状
2. 将需求分解为结构化的子任务 DAG
3. 子任务必须是已知 skill 名称加结构化参数的组合，委托板可校验合法性
4. 按拓扑顺序将子任务发布到委托板，指定执行者
5. 监控执行进度，处理异常

## 5. 记忆架构

Margatroid 的记忆系统模拟真实人类团队的文档习惯，分为两层：

### 5.1 工作日志（Worklog）

**团队共享的交接记录。** 每个委托创建时写入一行（summary 留空），产出时补全 summary。每条约 30-50 token，只记录：谁做的、委托给谁、做了什么。

工作日志是每个 agent 请求的**固定前缀**——通过 `Board.assemble_prompt()` 自动注入最近 20 条。不按需检索，始终注入，保证每个成员对团队状态有最低限度的感知。

### 5.2 个人记忆（Personal Memory）

**每个成员的私有笔记本。** 委托创建时插入一行（summary 留空），产出时补全 detail。存储委托的详细上下文（决策理由、遇到的问题和解决方案）。不限量，按需检索。

### 5.3 回忆机制（Recall）

**通过委托板直接搜索历史记忆。** 回忆被设计为一个工具（recall），每个 agent 默认拥有。调用 recall 工具时按关键词搜索 worklog + personal_memory，返回匹配条目。

### 5.4 存储模型

```
{workspace_root}/
├── .margatroid/
│   └── workspace/
│       └── {name}/
│           ├── memory.db       # 单文件，含 worklog + personal_memory + schedule + delegations
│           └── sandbox.toml    # 沙箱配置
```

### 5.5 上下文组装

每个 request 的上下文由 `Board.assemble_prompt(soul, memories)` 组装，固定顺序：

```
[人格提示词（System 消息）]
[系统提示词（User 消息）]
[委托链上下文]
[团队工作日志]
[个人记忆]
[当前任务]
```

## 6. 系统架构

### 6.1 Crate 结构

```
margatroid/
├── types/          # 共享类型定义（请求、响应、配置、消息、Tool、MCP、Bridge 协议）
├── paths/          # 路径布局管理（root、workspace、config、data）
├── assets/         # 统一资源管理器（app config + workspace 生命周期）
├── providers/      # AI 服务商适配层（trait + OpenRouter 实现）
├── server/         # HTTP 服务（Axum）
├── bridge/         # Claude Code Remote Control 协议客户端
├── cli/            # 命令行界面（margatroid 命令）
├── mcp_client/     # MCP 协议客户端
├── compose/        # compose 文件解析器、校验器、roster 生成器
├── sandbox/        # OS 原生沙箱（Linux: bwrap，macOS: sandbox-exec）
│   └── src/
│       ├── lib.rs / config.rs / mandatory.rs
│       ├── linux.rs / macos.rs
│       └── proxy/ (http.rs, socks5.rs)
├── runtime/        # 运行时核心（委托板、成员、记忆、Workspace、Client）
│   └── src/
│       ├── lib.rs          # re-exports
│       ├── board.rs        # DelegationBoard — 双区模型（发布 + 档案/SQLite）+ TaskChain + assemble_prompt
│       ├── client.rs       # Client — 封装 model + provider，统一聊天接口
│       ├── member.rs       # Member — 单成员封装（Agent trait，chat() tool-call loop）
│       ├── memory.rs       # SQLite 记忆（worklog + personal_memory + schedule + delegations）
│       └── workspace.rs    # Workspace — 成员生命周期、工具定义、控制循环
└── plugins/        # 插件系统骨架
```

### 6.2 数据流

```
用户 ──send_user_message──→ Workspace ──offer──→ [发布区]
                                                    │
              成员 take ─────────────────────────────┘
                              │
              成员 chat() ──→ bash / delegate / recall / schedule_*
                              │
              成员 finish ──→ result(done=true) ──→ 从发布区删除，写入档案(SQLite)
                         或 delegate ──→ result(done=false) + offer ──→ 新任务入发布区
```

### 6.3 Provider 架构

上层代码只依赖 `DynAiProvider` trait，不依赖具体实现。添加新服务商只需实现 `AiProvider` trait。错误统一使用 `anyhow::Result`。

```rust
pub trait AiProvider: Send + Sync {
    fn id(&self) -> &'static str;
    fn chat(&self, req: ChatRequest) -> impl Future<Output = Result<ChatResponse>>;
    fn chat_stream(&self, req: ChatRequest) -> impl Future<Output = Result<...>>;
}
```

runtime 通过 `Client` 封装 model + provider：

```rust
pub struct Client {
    model: String,
    provider: Arc<dyn DynAiProvider>,
}
```

## 7. API 端点

| 方法 | 路径          | 说明                         |
| ---- | ------------- | ---------------------------- |
| GET  | /health       | 健康检查                     |
| GET  | /v1/providers | 查询当前可用的 AI 服务商列表 |
| POST | /v1/chat      | 非流式 Chat 请求             |
| POST | /v1/stream    | SSE 流式 Chat 请求           |
| POST | /admin/reload | 热重载 provider 配置         |

待实现：

| 方法 | 路径                           | 说明                  |
| ---- | ------------------------------ | --------------------- |
| POST | /v1/workspace/create           | 创建 Workspace        |
| GET  | /v1/workspace/{id}             | 查询 Workspace 状态   |
| POST | /v1/workspace/{id}/task        | 向 Workspace 提交任务 |
| GET  | /v1/workspace/{id}/delegations | 查询委托板状态        |

## 8. 实现路线图

### Phase 1: Foundation
- [x] 基础 crate 结构（types, paths, assets, providers, server）
- [x] OpenRouter provider 适配（流式 + 非流式）
- [x] HTTP API 框架（/v1/chat, /v1/stream, /v1/providers, /admin/reload）
- [x] Bridge 远程控制协议
- [x] 项目从 AliceCode 更名为 Margatroid

### Phase 2: Compose & Workspace
- [x] compose 文件解析器（parser + validator + roster 生成器）
- [x] Workspace 生命周期管理（创建、列表、销毁）
- [x] OS 原生沙箱（bwrap / sandbox-exec，guard 守卫，HTTP 代理，强制保护路径）
- [x] 委托板（优先级队列、状态机、纠纷仲裁、cancel、PRIORITY_USER）
- [x] Agent 运行时（WorkspaceRuntime + AgentRuntime + engine 控制循环）
- [x] CLI（serve, compose validate/roster/load, workspace create/list）

### Phase 3: Memory & Recall（当前阶段）
- [x] 工作日志（Worklog）— 团队共享摘要，~30 token/条
- [x] 个人记忆（Personal Memory）— SQLite 实现
- [x] 回忆 Skill — 委托板关键词搜索 worklog + personal_memory
- [x] 委托板双区模型 — 发布区 + 档案区（SQLite）
- [x] TaskChain 图灵机模型 — delegate 右移，finish 左移
- [x] Agent 上下文构建 — Board.assemble_prompt() 六段拼接
- [x] tool-call loop — bash / delegate / finish / recall / schedule_*

### Phase 4: Production Readiness
- [ ] 完整测试覆盖（单元、集成、端到端）
- [ ] CLI 交互体验优化
- [ ] Human provider（人类伪装为 AI 模型供应商）
- [ ] streaming 支持
- [ ] 文档和示例 compose 文件

## 9. 设计决策日志

1. **项目命名** — Margatroid，取自东方 Project 角色 Alice Margatroid（七色的人形使）。原名 AliceCode 在项目从个人代码助手转型为多智能体编排框架后已不适用。CLI 命令为 `margatroid`，推荐别名 `mgt`。
2. **TOML 而非 YAML** — 与 Rust 生态配置传统一致，且 Margatroid 整体使用 TOML 作为配置格式。
3. **事件驱动而非轮询** — 避免委托板空转，降低资源消耗。
4. **两层成员发现** — Roster（轻量）+ Profile（按需），控制上下文窗口膨胀。
5. **工作日志 + 个人记忆双层模型** — 工作日志是团队共享的固定前缀（~30 token/条），每个人始终知道其他人在做什么。个人记忆是私有附录，按需检索。回忆通过委托板路由且不写入工作日志。
6. **回忆作为 skill 而非委托类型** — 每个 agent 默认拥有 recall skill，底层走委托板的 `skip_worklog` 通道。AgentRuntime 不需要区分任务类型。
7. **Manager 也是智能体** — 不是硬编码调度器，保持系统灵活性。
8. **depends_on 仅自文档化** — 不强制启动顺序，协作关系运行时动态建立。
9. **OS 原生沙箱而非 Docker/MicroVM** — 利用 bubblewrap（Linux）和 sandbox-exec（macOS）做进程级隔离。Agent 直接继承宿主工具链。沙箱环境临时可丢弃，只有 workdir 和 memory.db 持久化。
10. **仅支持 Linux 和 macOS** — 不兼容 Windows。代码中不使用任何 Windows 条件编译或兼容层。

## 10. 沙箱参考设计与技术栈

### 10.1 参考项目

Margatroid 的沙箱架构直接借鉴 Anthropic 开源的 `sandbox-runtime`（Claude Code 的沙箱运行时），并结合 Rust 生态中的现有 crate 设计。

| 参考项目 | 语言 | 参考价值 |
|---|---|---|
| `@anthropic-ai/sandbox-runtime` | TypeScript | 核心架构：SandboxManager、双代理模型、强制保护路径、配置格式 |
| `astrid-workspace` | Rust | macOS Seatbelt profile 动态生成、路径注入安全校验、`sandbox-exec` 参数构造 |
| `extrasafe` | Rust | Linux seccomp BPF 过滤规则生成（可选防御纵深） |

### 10.2 依赖清单

| Crate | 用途 |
|---|---|
| `tokio` | 异步运行时、进程管理、代理服务器 I/O |
| `serde` / `serde_json` | 沙箱配置的序列化与反序列化 |
| `tracing` | 日志和违规事件追踪 |
| `tempfile` | macOS Seatbelt 配置临时文件 |
| `which` | 检测 `bwrap` / `sandbox-exec` 是否可用 |
| `hyper` + `http` | HTTP/HTTPS 代理服务器实现 |
| `tokio-socks` | SOCKS5 代理实现 |
| `hickory-resolver` | DNS 解析（域名白名单校验） |

### 10.3 架构设计要点

**统一 trait 接口：**

```rust
pub trait Sandbox: Send + Sync {
    /// 初始化沙箱（启动代理服务器等）
    async fn initialize(&mut self, config: SandboxConfig) -> Result<()>;

    /// 将任意 shell 命令包装为沙箱化命令
    fn wrap_command(&self, cmd: &str) -> String;

    /// 重置沙箱（停止代理、清理临时文件）
    async fn reset(&mut self) -> Result<()>;
}
```

**平台实现：**
- `LinuxSandbox` — 调用 `bwrap`，配置 `--unshare-all`、`--bind` 挂载、`--seccomp` BPF 过滤
- `MacOSSandbox` — 调用 `/usr/bin/sandbox-exec`，动态生成 Seatbelt profile（SBPL 格式）

**防御纵深原则：**
- 基础层：命名空间隔离（bwrap）或 Seatbelt 策略（macOS）
- 增强层：seccomp BPF 系统调用过滤（Linux，可选）
- 网络层：HTTP/SOCKS5 代理域名白名单
- 代码层：强制保护路径硬编码，不受配置影响

### 10.4 从 Claude Code 借鉴的五项设计约束

1. **写入默认全禁，网络默认全禁** —— allow-only 模型，最小权限原则
2. **强制保护路径** —— `~/.ssh`、`~/.aws`、`.env`、`.gitconfig`、`.git/hooks/`、`.mcp.json` 硬编码禁止写入
3. **双代理网络过滤** —— HTTP 代理处理 Web 流量，SOCKS5 代理处理其余 TCP
4. **工具链不打包** —— agent 直接继承宿主系统工具（rustc、node、python 等）
5. **防绕过** —— `allowUnsandboxedCommands: false` 防止 agent 禁用沙箱
