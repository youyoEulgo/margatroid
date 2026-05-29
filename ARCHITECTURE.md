# Margatroid 架构文档

## 概述

Margatroid 是一个用 Rust 实现的多 Agent 协作运行时，模拟人类团队的工作方式：通过委托（Delegation）机制分发任务，成员在沙箱中执行。

核心设计理念是将委托过程建模为**任务链（TaskChain）**，类似图灵机的纸带：读写头在链上移动，委托时右移、完成时左移，上下文即整条链。

## Provider 隔离

runtime crate 完全不接触 provider 实现。`DynAiProvider` trait 定义在 `types` crate 中，`providers` crate 负责实现和解析。

```
types::DynAiProvider   ← runtime 通过 Client 持有，通过它做 LLM 调用
types::AiProvider      ← providers 实现，blanket impl → DynAiProvider
providers::resolve()   ← 按 provider 名称从配置自动查找并构建
providers::build()     ← 按 provider_type 分发构造
runtime::Client        ← 封装 model + provider，统一聊天/流式接口
```

支持的 Provider：`OpenRouterProvider`、`DeepSeekProvider`（直连 API，OpenAI 兼容格式）、`HumanProvider`（人类交互回退）。所有 provider 层错误统一使用 `anyhow::Error`。

## 任务链（TaskChain）

### 数据结构

```rust
enum ChainEntry {
    Delegate { task: DelegationTask, parent_idx: usize },
    Outcome { result: TaskResult, delegate_idx: usize },
}
struct TaskChain {
    entries: Vec<ChainEntry>,  // 链（只追加）
    head: usize,               // 读写头：当前活跃委托位置
}
```

### 操作

- **delegate（右移）**：`entries.push(Delegate { parent_idx: head })` → `head = entries.len() - 1`
- **finish（左移）**：`entries.push(Outcome { delegation_id, done })` → `done=true` 时 `head = entries[head].parent_idx`
- **上下文**：`entries[0..]` 就是从根到当前的全部记录，由 `assemble_prompt()` 转为 LLM 上下文
- **链只追加，不删改**：任何 Delegate 或 Outcome 写入后永不修改，链是调度和上下文的唯一权威源

### 多点 Outcome

一个委托可以产出多个 Outcome，配合子委托形成工作流：

```
Delegate(d1, user→manager)        head=0
Outcome(d1, "阶段性产出", done:false) head=0  ← manager 需要帮助
Delegate(d2, manager→coder)       head=2  ← coder 接手
Outcome(d2, "完成", done:true)     head=0  ← 回到 manager
Outcome(d1, "最终版", done:true)   head=0  ← 完成
```

## 委托板（Delegation Board）

任务链驱动调度，发布区降级为前端可视化缓存。

```
offer → [发布区(缓存)] + 链新增 Delegate
链. current_task() → 成员循环读取 → 匹配则执行
result(done=true) → 链新增 Outcome + 发布区移除 → 唤醒上级
```

- **发布区**：`offer` 写入，`result(done=true)` 移除归档。只读不删（`take()` 为只读查询）。前端轮询 /ws/{name}/status 读取 publish_count
- **调度机制**：`member_loop` 直接读 `chain.current_task()`，当 `task.to == agent.id()` 时领取执行。不再依赖发布区
- **事件驱动唤醒**：每成员持 `tokio::sync::Notify`。`offer()` 和 `result(done=true)` 在链头移动后唤醒目标成员。不再轮询
- **重试**：`execute_task` 错误路径自动 re-offer（`[RETRY:N]` 前缀），超过 3 次放弃

## 成员（Member）

每个成员封装了 Client、SOUL 提示词、沙箱访问。

```rust
pub struct Member {
    pub id: String,
    soul: String,
    identity: Identity,
    client: Client,
    sandbox: Arc<RwLock<SandboxManager>>,
}
```

三种 Identity：`User`（不 spawn 循环）、`Manager`（额外 schedule_* 工具）、`Member`（base 工具）。

### Tool-Call 循环（流式）

`chat()` 通过 `Client::chat_stream()` 获取流，每收一个 chunk：
1. 原样 `publish_raw` 推 SSE 给前端（透传不做加工）
2. 累积 `full_content` / `full_tool_calls` / `finish_reason`
3. 流结束后保存完整文本到 `conversation_messages` 表
4. 非破坏工具（bash/recall/schedule_*）→ 执行后继续循环
5. `finish` → 产出结果（done=true），推 `{"type":"done"}` SSE 事件
6. `delegate` → 记录阶段性产出（done=false），发布新委托，推 done 事件
7. `ChatMessage` / `ResponseDelta` / `ResponseMessage` 带 `reasoning_content: Option<String>` 字段

### Tool-Call 合并

流式 tool call 的 arguments 分片发送，`merge_deltas` 按 `ToolCallDelta.index` 位置合并增量

## SSE 实时推送

`GET /ws/{name}/events/{task_id}` 建立 SSE 连接，后端把 LLM 返回的每个 `StreamChunk` JSON 原样透传给前端。`execute_finish` 结束时推送 `{"type":"done"}` 关闭连接。

底层通过 `DelegationBoard` 的 `broadcast::Sender<String>` 实现——`offer()` 时预建 channel，`publish_raw()` 写入，`BroadcastStream` 读出。连接时先检查 worklog 防竞态。

## 上下文组装

`Board.assemble_prompt(soul, memories)` 按以下顺序组装 LLM 上下文：

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

SQLite 实时写保证持久化。`DelegationBoard` 持 `cached_worklog: RwLock<String>`，只在启动时和根委托完成时刷新。子任务执行期间缓存不变，保持 LLM 上下文稳定，最大化 prompt cache 命中。

## 记忆系统（SQLite）

单文件 `memory.db`，含五张表：

- **worklog**：团队工作日志，委托创建时插入，产出时补全 summary/reply
- **personal_memory**：个人记忆，委托创建时插入，产出时补全 detail
- **conversation_messages**：对话消息，每次 LLM 文本回复实时存入。`GET /ws/{name}/conversation` 可查询
- **schedule**：Manager 专用，planned/offered/archived 状态流转
- **delegations**：委托持久化，创建时写入完整信息

## 日志体系

四层日志：

| 级别 | 默认 | 开启方式 | 内容 |
|---|---|---|---|
| error | 是 | — | 不可恢复错误 |
| warn | 是 | — | chunk 跳过、流降级、任务重试 |
| info | 是 | — | 成员启动、任务领取、publish done、board 变动 |
| debug | 否 | `--verbose` 或 `level = "debug"` | 格式化摘要 + 原始 JSON 全文 |

`--verbose` 独立的 info 层守卫（成员名+工具名摘要），debug 级别打原始流量不做任何加工。日志级别从 `margatroid.toml` 的 `[logging] level` 读取，大小写不敏感。

## 成员名录注入

`Workspace::start()` 从 `AgentEntry.skills` 拼成员名单，注入 `DelegationBoard.member_roster`，`assemble_prompt()` 在系统提示词后插入。格式：

```
--- 团队成员 ---
- manager (经理) — 技能: manage, planning, delegation
- coder (成员) — 技能: coding, bash
- reviewer (成员) — 技能: review, testing
```

## 项目结构

```
margatroid/
├── types/         # 共享类型定义（DynAiProvider, AiProvider, Identity, ChatRequest 等）
│   └── provider.rs  # DynAiProvider + AiProvider trait + blanket impl
├── runtime/       # 核心运行时（Workspace, DelegationBoard, Member, TaskChain, Client, memory）
│   ├── board.rs   # DelegationBoard + TaskChain + assemble_prompt + Notify
│   ├── client.rs  # Client — 封装 model + provider + 流式/降级 + verbose 日志
│   ├── member.rs  # Member — Agent trait + chat() 流式 tool-call loop
│   ├── memory.rs  # SQLite（worklog + personal_memory + schedule + delegations + conversation_messages）
│   └── workspace.rs  # Workspace + 工具定义 + member_loop 事件驱动
├── providers/     # LLM 供应商（OpenRouter + DeepSeek + Human）+ resolve()/build()
├── compose/       # Compose 文件解析
├── assets/        # 成员库（member.toml + SOUL.md）+ 系统提示词管理
├── sandbox/       # 沙箱执行环境
├── cli/           # 命令行入口 + --verbose + 配置日志等级
└── server/        # HTTP API（axum）+ SSE + CORS + ws 路由
```

## Workspace 生命周期

```rust
Workspace::start(compose, entries) → 创建 Board、沙箱、SQLite，spawn 成员控制循环
send_user_message(from, to, brief, detail) → 发布根委托到发布区 + 链
shutdown() → 通知所有成员退出，await 所有 handle
```
