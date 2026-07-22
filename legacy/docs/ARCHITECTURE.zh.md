# Margatroid 架构设计

> 项目名取自东方 Project 角色 Alice Margatroid，称号"七色的人形使"，能力是同时操控多个人偶作战。在 Margatroid 中，用户是 AI 人形使，compose 文件定义每个智能体的能力和角色，委托板是智能体之间的沟通通道。

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

| 要素             | 说明                                               |
| ---------------- | -------------------------------------------------- |
| Provider + Model | 底层 AI 模型（OpenRouter / DeepSeek 直连 / Human） |
| Skills           | 该实例具备的能力集合（函数/工具定义）              |
| SOUL.md          | 角色身份和行为边界                                 |
| 记忆系统         | worklog + personal_memory                          |

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

| 操作              | 效果                     |
| ----------------- | ------------------------ |
| `schedule_add`    | 添加 planned 条目        |
| `schedule_list`   | 列出所有 planned 条目    |
| `schedule_pop`    | 弹出某成员最高优先级条目 |
| `schedule_remove` | 删除指定条目             |

一个阶段任务 = 一个成员被分配的顶层工作项。阶段任务通过 board 发布为委托，完成后 Manager accept 时归档。

### 2.6 沙箱（Sandbox）

OS 原生沙箱方案，利用操作系统内核级隔离机制：

| 平台  | 隔离机制                                                          |
| ----- | ----------------------------------------------------------------- |
| Linux | Bubblewrap (`bwrap`) — 挂载命名空间 + PID 命名空间 + 网络命名空间 |
| macOS | `sandbox-exec` — 动态生成的 Seatbelt 配置文件                     |

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

## 5. Tool-Call 循环（流式 / 降级）

`chat()` 通过 `Client::chat_stream()` 获取流，每收一个 chunk：
- 解析为 `StreamChunk`（失败则空壳），构造 `WorkspaceEvent { content: StreamChunk { chunk } }`
- 通过 `send_event()` 推到 `{workspace}/stream` EventBus 通道
- 累积 full_content / full_tool_calls / finish_reason / full_reasoning
- 非流式降级：StreamChunk 解析失败时按 `ChatResponse` 解析（`message` 替代 `delta`），合并到累积
- 流结束后保存完整文本到 conversation_messages
- 非破坏工具（bash/recall/schedule_*）→ 执行后继续循环
- finish → 产出（done=true），发 chain_update 事件，break 退出
- delegate → 产出（done=false），发新委托，break 退出

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

Margatroid 通过全局 EventBus 统一管理事件通道，命名格式 `<workspace>/stream`。

### 7.1 统一事件流

前端通过 `GET /workspace/{name}/stream` 长期订阅，所有事件走同一条通道。事件结构为 `metadata` + `content`：

```json
{
  "metadata": { "event": "stream_chunk", "member_id": "manager", "delegation_id": "...", "timestamp": 123 },
  "content": { "chunk": { "id": "...", "model": "...", "choices": [...] } }
}
```

五类事件：

| `metadata.event` | 触发时机                    | `content`                                 |
| ---------------- | --------------------------- | ----------------------------------------- |
| `stream_chunk`   | LLM 流式输出每 chunk        | `{ chunk: StreamChunk }`                  |
| `board_update`   | offer / result(done=true)   | `{ publish_count: number }`               |
| `chain_update`   | delegate 右移 / finish 左移 | `{ from, to, brief, head_pos }`           |
| `member_status`  | 成员开始/结束处理           | `{ state: "working" \| "idle" }`          |
| `human_request`  | HumanProvider 创建请求      | `{ session_id, from, to, brief, detail }` |

### 7.2 Rust 定义

`types/src/events.rs`：

```rust
pub struct WorkspaceEvent {
    pub metadata: EventMetadata,  // event, member_id, delegation_id, timestamp
    pub content: EventContent,    // #[serde(untagged)] — 不加 type tag
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum EventContent {
    StreamChunk { chunk: StreamChunk },     // 引用 types::StreamChunk
    BoardUpdate { publish_count: usize },
    ChainUpdate { from, to, brief, head_pos },
    MemberStatus { state: String },
    HumanRequest { session_id, from, to, brief, detail },
}
```

### 7.3 EventBus

`runtime_v2/src/events.rs` — 全局 `EventBus`（`HashMap<String, broadcast::Sender<String>>`）。`Workspace::new()` 注册 `"{name}/stream"` 通道，`Member::send_event()` 构造 `WorkspaceEvent` 并发送。不再有三通道模型和 event_bridge 中间层——runtime 直接构造类型化事件。

### 7.4 前端分派

前端按 `metadata.event` 分流，`content` 直接对应 TypeScript 类型：

```ts
switch (metadata?.event) {
  case 'stream_chunk': handleStreamChunkEvent(content, metadata.member_id); break;
  case 'board_update': handleBoardUpdateEvent(content); break;
  case 'chain_update': handleChainUpdateEvent(content); break;
  case 'member_status': handleMemberStatusEvent(content, metadata.member_id); break;
  case 'human_request': handleHumanRequestEvent(content); break;
}
```

五个 handler 各接收对应类型，零处理直传。

### 7.5 流式 UI

收到第一个 `stream_chunk` 立即 push 消息到 messages，后续 chunk 实时追加。`member_status: idle` 时 flush tool 结果并置 `loading = false`。

### 7.6 推送点

| 事件                        | 触发位置                        | 层         |
| --------------------------- | ------------------------------- | ---------- |
| stream_chunk                | `member.chat()` while loop      | runtime_v2 |
| board_update + chain_update | `trigger_event()`               | runtime    |
| chain_update                | `member.chat()` should_break    | runtime_v2 |
| member_status               | `engine::member_loop` 开始/结束 | runtime_v2 |
| human_request               | `human::handle_request`         | server     |

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

| 方法 | 路径                               | 说明                         |
| ---- | ---------------------------------- | ---------------------------- |
| GET  | /health                            | 健康检查                     |
| POST | /v1/chat                           | 非流式 Chat                  |
| POST | /v1/stream                         | SSE 流式 Chat                |
| GET  | /v1/providers                      | AI 服务商列表                |
| POST | /admin/reload                      | 热重载 provider              |
| GET  | /workspace                         | 列出运行中的 workspace       |
| POST | /workspace/{name}/chat             | 向 Workspace 发消息          |
| GET  | /workspace/{name}/status           | 委托板发布区计数             |
| GET  | /workspace/{name}/tasks            | 委托板完整状态               |
| GET  | /workspace/{name}/events/{task_id} | Per-task SSE 事件流          |
| GET  | /workspace/{name}/stream           | Workspace 统一事件流（长期） |
| GET  | /workspace/{name}/recent           | 最近工作日志                 |
| GET  | /workspace/{name}/conversation     | 对话消息                     |
| POST | /api/human/request                 | 人类交互请求                 |
| GET  | /api/human/request/{id}            | 阻塞等待人类回复             |
| GET  | /api/human/requests                | 待处理请求列表               |
| POST | /api/human/request/{id}/reply      | 提交人类回复                 |

## 11. 项目结构

```
margatroid/
├── types/         # 共享类型定义（DynAiProvider, AiProvider, Identity, ChatRequest, EventMetadata, EventContent 等）
│   ├── provider.rs  # DynAiProvider + AiProvider trait + blanket impl
│   ├── events.rs    # WorkspaceEvent = EventMetadata + EventContent（#[serde(untagged)]）
│   └── event_index.rs # 事件名常量
├── runtime/       # V1 核心运行时
│   ├── board.rs   # DelegationBoard + TaskChain + assemble_prompt + trigger_event
│   ├── client.rs  # Client — 封装 model + provider + 流式/降级
│   ├── member.rs  # Member — Agent trait + chat() 流式 tool-call loop
│   ├── memory.rs  # SQLite 五表
│   └── workspace.rs  # Workspace 生命周期 + member_loop
├── runtime_v2/    # V2 运行时（逐步迁移中）
│   ├── engine.rs  # member_loop（事件驱动 + send_event 推 EventBus）
│   ├── board.rs   # DelegationBoard V2 — 纯调度层
│   ├── member.rs  # Member V2 — Agent trait（持有 EventBus 引用）
│   ├── events.rs  # EventBus — HashMap<broadcast::Sender> 通道注册表
│   ├── workspace.rs # Workspace V2 — 接收 EventBus，管理成员生命周期
│   ├── context.rs # 上下文组装（assemble_prompt + worklog）
│   ├── memory.rs  # SQLite
│   ├── tools.rs   # 工具执行
│   └── kernel.rs  # Kernel — 多 workspace 进程级宿主
├── providers/     # LLM 供应商（OpenRouter + DeepSeek + Human）
├── compose/       # Compose 文件解析
├── assets/        # 成员库（member.toml + SOUL.md）
├── sandbox/       # 沙箱执行环境
├── cli/           # 命令行入口
└── server/        # HTTP API（axum）+ SSE + workspace 路由
    ├── human.rs        # Human 交互端点
    └── handlers/       # stream, chat, workspace, providers, admin
```

## 12. 设计决策日志

1. **任务链只追加不删改** — delegate 和 Outcome 写入后永不修改，链是调度和上下文的唯一权威源
2. **事件驱动** — Notify + 链头检测替代轮询，零 CPU 空转
3. **metadata.event 分派 + content untagged** — 前端按 metadata.event 分派，content 为裸数据直传 TypeScript 类型，零处理
4. **单通道 EventBus** — 废弃三通道模型 + event_bridge，workspace 统一 `{name}/stream` 通道，runtime 直接构造类型化 JSON
5. **工作日志缓存** — 启动+根完成刷新，中间不变保 prompt cache
6. **Manager 也是智能体** — 不是硬编码调度器
7. **发布区降级** — 前端缓存，不影响调度逻辑
8. **两层成员发现** — 团队成员名录（轻量注入上下文）+ recall 按需检索
9. **三层日志体系** — error/warn 默认开，info 业务节点，debug 原始流量
10. **身份路由** — CLI 中 match identity 三路独立
11. **Human 成员** — User 配 HumanProvider，用户既是链发起者也是可委托对象
12. **流式 UI** — 首 chunk 即 push 消息，后续实时追加，idle 时 flush tool 并置 loading
