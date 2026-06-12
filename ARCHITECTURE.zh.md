# Margatroid 架构设计

> 项目名取自东方 Project 角色 Alice Margatroid，称号"七色的人形使"，能力是同时操控多个人偶作战。在 Margatroid 中，用户是 AI 人形使，compose 文件定义每个智能体的能力和角色，委托板是连接智能体与用户的丝线。

## 1. 设计哲学

Margatroid 采用**人类团队协作模型**作为设计基础。一个 Workspace 就是一个项目小组，每个 AI 智能体是小组中拥有特定技能的成员，Manager 是项目经理，委托板是团队的协作基础设施（任务链 + 发布区 + SQLite 归档）。

## 2. 核心概念

### 2.1 Workspace

一个独立的、沙箱化的协作环境，包含：
- 一组 AI 智能体实例（由 compose 文件定义）
- 一个委托板（Delegation Board + TaskChain）
- 一个 SQLite 数据库（worklog + personal_memory + schedule + delegations + conversation_messages）
- 共享沙箱环境

### 2.2 AI 智能体实例

由以下要素构成：

| 要素 | 说明 |
|------|------|
| Provider + Model | 底层 AI 模型（OpenRouter / DeepSeek 直连 / Human）|
| Skills | 该实例具备的能力集合（函数/工具定义）|
| SOUL.md | 角色身份和行为边界 |
| 记忆系统 | worklog + personal_memory |

### 2.3 AI Manager

Manager 是 Workspace 中的特殊成员（Identity::Manager），角色等同于项目经理：
- 直接与用户对接，接收用户需求
- 将用户需求分解为可委派的结构化任务
- 通过委托板将任务分发给合适的智能体
- 审查返回结果，合格则 finish，不合格则 finish 说明原因
- Manager 本身也是调用 LLM 的智能体，拥有 schedule_* 额外工具

### 2.4 委托板（Delegation Board + TaskChain）

委托板是 Workspace 的核心协作基础设施，由任务链驱动：

```
offer → 链新增 Delegate + 发布区（前端缓存）
链 current_task() → 成员循环读取 → to 匹配则执行
result(done=true) → 链新增 Outcome + 发布区移除 + 唤醒上级
```

**任务链（TaskChain）** 是图灵机模型——链只追加不删改。delegate 右移，finish(done=true) 左移。上下文即整条链。

**发布区** 已降级为前端可视化缓存，成员循环不再依赖它。调度完全由链的 `current_task()` 驱动。

**事件驱动唤醒**：每成员持 `tokio::sync::Notify`。`offer()` 和 `result(done=true)` 在链头移动后唤醒目标成员。不再使用轮询。

**用户消息**：通过 `send_user_message("user", "manager", ...)` 以根委托发布到链。

**重试**：成员执行失败时任务自动重新发布（detail 前缀 `[RETRY:N]`），超过 3 次放弃。

### 2.5 计划表（Schedule）

Manager 专用的阶段任务规划工具，存于 board 的 SQLite。

| 操作 | 效果 |
|------|------|
| `schedule_add` | 添加 planned 条目 |
| `schedule_list` | 列出所有 planned 条目 |
| `schedule_pop` | 弹出某成员最高优先级条目 |
| `schedule_remove` | 删除指定条目 |

一个阶段任务 = 一个成员被分配的顶层工作项。阶段任务通过 board 发布为委托，完成后 Manager accept 时归档。

### 2.6 沙箱（Sandbox）

OS 原生沙箱方案，利用操作系统内核级隔离机制：

| 平台 | 隔离机制 |
|------|----------|
| Linux | Bubblewrap (`bwrap`) — 挂载命名空间 + PID 命名空间 + 网络命名空间 |
| macOS | `sandbox-exec` — 动态生成的 Seatbelt 配置文件 |

写入默认全禁（allow-only），网络默认全禁。强制保护路径硬编码（~/.ssh、~/.aws、.env、.gitconfig 等）。

每个 Workspace 分配独立沙箱。沙箱环境临时可丢弃，只有 workdir 和 memory.db 持久化。

## 3. 委托流程

```
1. 成员 A 发现自己需要 B 的能力
2. A 通过团队成员名录识别 B 可能合适
3. A 调用 delegate 工具，指定 target、task_summary、task_detail，
   附带 work_summary/work_detail
4. board 记录 A 的产出（done=false），链新增子委托，唤醒 B
5. B 通过链 current_task() 领取任务，执行，完成后调用 finish
6. board 记录 B 的产出（done=true），链头左移回 A，唤醒 A
7. A 审查 B 的结果，决定 accept（finish）或指出问题（finish 说明原因）
```

## 4. Human 交互（用户成员）

用户作为团队成员参与委托链。Identity::User 成员配 HumanProvider，Manager 可 delegate 给用户。HumanProvider 通过 POST /api/human/request 创建请求、阻塞等待回复。前端通过 SSE workspace_stream 接收 human_request 事件，输入框自动切换为回复模式，提交 reply 后链继续走。

## 5. Tool-Call 循环（流式）

`chat()` 通过 `Client::chat_stream()` 获取流（降级：流失败时用非流式包裹为单条），每收一个 chunk：
- 原样 `publish_raw` 推 SSE 给前端（透传不做加工）
- 累积 full_content / full_tool_calls / finish_reason / full_reasoning
- 非破坏工具（bash/recall/schedule_*）→ 执行后继续循环
- finish → 产出（done=true），推 `{"type":"done"}` SSE 事件
- delegate → 产出（done=false），发新委托，推 done 事件

流式 tool call 的 arguments 分片发送，`merge_deltas` 按 `ToolCallDelta.index` 合并增量。

## 6. 上下文组装

`Board.assemble_prompt(soul, memories)` 按固定顺序组装：

```
1. 系统提示词（User）
2. 团队成员名录（User）
3. 团队工作日志（User）— 从内存缓存读取
4. 委托链上下文（User）
5. 人格提示词（System）— SOUL.md
6. 个人记忆（User）
7. 当前任务（User）— 动态，始终最后
```

### 工作日志缓存

SQLite 实时写，但 `DelegationBoard` 持内存缓存。只在启动和根委托完成时刷新。子任务执行期间不变，保持 LLM 上下文稳定。

## 7. 事件系统

Margatroid 事件系统由三条 broadcast 通道、四类结构事件、一条事件桥接组成。

### 7.1 三通道模型

所有通道都是 `broadcast::Sender<String>`，存于 `DelegationBoard.events: HashMap<String, Sender>`。

| 通道 | key | 创建时机 | 写入者 | 读取者 | 生命周期 |
|------|-----|----------|--------|--------|----------|
| per-task | task_id (UUID) | 每次 offer() 动态创建 | chat() | 前端 taskEs | task 结束即废弃 |
| workspace_stream | 固定字符串 | board 构造时预建 | event_bridge, human.rs | 前端 wsStream | workspace 存活期间 |
| raw_streams | 固定字符串 | board 构造时预建 | chat() | 前端 wsStream | workspace 存活期间 |

#### 7.1.1 per-task channel

LLM 流式输出的专属通道。`chat()` 每收一个 chunk，原样 `publish_raw(task_id, chunk_json)`。前端通过 `GET /workspace/{name}/events/{task_id}` 订阅。只有发起对话端监听——用户发消息拿到的 task_id 即 manager 的通道，coder/reviewer 等子委托的 per-task 通道无前端监听。

推送内容为原始 LLM chunk，不经任何加工。`{"type":"done"}` 由 `chat()` 在 finish/delegate 时插入，标记 LLM 轮次边界。

#### 7.1.2 workspace_stream channel

全局结构化事件通道。前端通过 `GET /workspace/{name}/stream` 长期订阅。承载四类事件：

| 事件 type | 触发 | 携带数据 |
|-----------|------|----------|
| board_update | offer / result(done=true) | publish_count: usize |
| chain_update | delegate 右移 / finish 左移 | from, to, brief, head_pos |
| member_status | execute_task 开始/结束 | member_id, state (working/idle) |
| human_request | HumanProvider 创建请求 | session_id, from, to, brief, detail |

#### 7.1.3 raw_streams channel

所有成员 LLM 输出的共享通道。`chat()` 每收一个 chunk，在写 per-task 的同时复制一份到 raw_streams，包装为 `{type:"stream_chunk", member_id, chunk}`。前端同一 SSE 连接收到后按 member_id 累计，member_status idle 时刷出为一条消息。

不经过 event_bridge——chat() 直接用 `publish_raw` 写入，透传即可。

### 7.2 双流合并

`server/src/handlers/workspace.rs` 的 `stream()` handler 同时订阅 workspace_stream 和 raw_streams：

```rust
let structured = BroadcastStream::new(rx_ws);    // 结构事件
let raw        = BroadcastStream::new(rx_raw);   // 成员 chunk
let merged     = stream::select(structured, raw); // 合并为一个 SSE
```

前端一个 EventSource 连接接收两类消息，按 `event.type` 分派。

### 7.3 事件桥接

`server/src/event_bridge.rs` — 每 workspace 一个 tokio task，订阅 CHANNEL_RAW_EVENTS，按 `event_name` 分派：

| 事件名 | 处理 |
|--------|------|
| board_update | BoardUpdateEvent::new(count) → JSON |
| chain_update | 查链当前 task → ChainUpdateEvent::new() → JSON |
| member_status | 解析 member_id+state → MemberStatusEvent::new() → JSON |

runtime 层通过 `trigger_event()` 写入原始事件名+数据到 raw_events，不构造 JSON、不依赖 types::events。event_bridge 完成从"事件名"到"类型化 JSON"的转换，推送 workspace_stream。

human_request 不在桥接中——它由 `server/src/human.rs` 直接写 workspace_stream，因为触发点在 server 层。

### 7.4 推送点汇总

| 事件 | 触发位置 | 层 |
|------|----------|-----|
| board_update + chain_update | `board.offer()` | runtime |
| board_update + chain_update | `board.result(done=true)` | runtime |
| chain_update | `board.result(done=false)` | runtime |
| member_status | `execute_task` 开始/结束 | runtime |
| human_request | `human::create_request` handler | server |
| stream_chunk | `chat()` while 循环 | runtime |

## 8. 记忆系统（SQLite）

单文件 `memory.db`，含五张表：
- **worklog** — 团队工作日志，委托创建时插入，产出时补全
- **personal_memory** — 个人记忆，按需关键词检索
- **conversation_messages** — 对话消息，LLM 每次文本回复实时写入
- **schedule** — Manager 专用计划表
- **delegations** — 委托持久化

## 9. Provider 隔离

```
types::DynAiProvider   ← runtime 通过 Client 持有
types::AiProvider      ← providers 实现
runtime::Client        ← 封装 model + provider
```

支持的 Provider：`OpenRouterProvider`、`DeepSeekProvider`（直连 API，OpenAI 兼容格式）、`HumanProvider`。

## 10. API 端点

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | /health | 健康检查 |
| POST | /v1/chat | 非流式 Chat |
| POST | /v1/stream | SSE 流式 Chat |
| GET | /v1/providers | AI 服务商列表 |
| POST | /admin/reload | 热重载 provider |
| GET | /workspace | 列出运行中的 workspace |
| POST | /workspace/{name}/chat | 向 Workspace 发消息 |
| GET | /workspace/{name}/status | 委托板发布区计数 |
| GET | /workspace/{name}/tasks | 委托板完整状态 |
| GET | /workspace/{name}/events/{task_id} | Per-task SSE 事件流 |
| GET | /workspace/{name}/stream | Workspace 统一事件流（长期） |
| GET | /workspace/{name}/recent | 最近工作日志 |
| GET | /workspace/{name}/conversation | 对话消息 |
| POST | /api/human/request | 人类交互请求 |
| GET | /api/human/request/{id} | 阻塞等待人类回复 |
| GET | /api/human/requests | 待处理请求列表 |
| POST | /api/human/request/{id}/reply | 提交人类回复 |

## 11. 项目结构

```
margatroid/
├── types/         # 共享类型定义（DynAiProvider, AiProvider, Identity, ChatRequest 等）
│   ├── provider.rs  # DynAiProvider + AiProvider trait + blanket impl
│   ├── events.rs    # 事件 struct（BoardUpdate, ChainUpdate 等）
│   └── event_index.rs # 通道/事件名常量
├── runtime/       # 核心运行时（Workspace, DelegationBoard, Member, TaskChain, Client, memory）
│   ├── board.rs   # DelegationBoard + TaskChain + assemble_prompt + Notify + SSE 广播
│   ├── client.rs  # Client — 封装 model + provider + 流式/降级 + verbose 日志
│   ├── member.rs  # Member — Agent trait + chat() 流式 tool-call loop
│   ├── memory.rs  # SQLite 五表
│   └── workspace.rs  # Workspace 生命周期 + member_loop 事件驱动
├── providers/     # LLM 供应商（OpenRouter + DeepSeek + Human）+ resolve()/build()
├── compose/       # Compose 文件解析
├── assets/        # 成员库（member.toml + SOUL.md）+ 系统提示词管理
├── sandbox/       # 沙箱执行环境
├── cli/           # 命令行入口 + --verbose + 配置日志等级 + 身份路由
└── server/        # HTTP API（axum）+ SSE + CORS + workspace 路由
    ├── event_bridge.rs # runtime raw_events → frontend workspace_stream
    └── human.rs        # Human 交互端点
```

## 12. 设计决策日志

1. **任务链只追加不删改** — delegate 和 Outcome 写入后永不修改，链是调度和上下文的唯一权威源
2. **事件驱动** — Notify + 链头检测替代轮询，零 CPU 空转
3. **流式透传** — SSE 不过加工，前端自解析。类型系统用 ToolCallDelta（全可选）适配增量
4. **工作日志缓存** — 启动+根完成刷新，中间不变保 prompt cache
5. **Manager 也是智能体** — 不是硬编码调度器
6. **发布区降级** — 前端缓存，不影响调度逻辑
7. **两层成员发现** — 团队成员名录（轻量注入上下文）+ recall 按需检索
8. **三层日志体系** — error/warn 默认开，info 业务节点，debug 原始流量
9. **事件桥接** — runtime 触发事件名，server 构造类型化消息，不改 runtime 依赖
10. **身份路由** — CLI 中 match identity 三路独立，清晰可维护
11. **Human 成员** — User 配 HumanProvider，用户既是链发起者也是可委托对象
