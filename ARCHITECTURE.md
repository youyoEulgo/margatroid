# Margatroid 架构文档

## 概述

Margatroid 是一个用 Rust 实现的多 Agent 协作运行时，模拟人类团队的工作方式：通过委托（Delegation）机制分发任务，成员在沙箱中执行，审查结果后归档。

## 核心概念

### 成员（Member）

每个成员封装了 LLM 模型、系统提示词（SOUL.md）、沙箱访问和工具集。成员不区分身份——User、Manager、Member 三种 Identity 仅在成员库（member library）中标记，Workspace 层面对所有成员一视同仁。

成员通过 `Agent` trait 暴露统一接口：

```rust
async fn process(&self, prompt: &str, task_description: &str, tools: &[RequestTool]) -> Result<ChatOutcome>;
```

### 委托板（Delegation Board）

四区状态机模型，全异步、无阻塞：

```
offer ──→ [发布区] ──claim──→ [执行区] ──return──→ [返回区] ──accept──→ [档案区: SQLite]
              ↑                                       │       │
              └────────── reject ─────────────────────┘       │
                                                       accept → [档案区]
```

- **发布区（publish）**：待领取的任务，按 priority 降序排列
- **执行区（exec）**：成员正在执行的任务
- **返回区（returned）**：已完成、等待发布者审核
- **档案区（SQLite）**：已 accept 的任务写入 worklog + personal_memory

### 委托任务（DelegationTask）

```rust
pub struct DelegationTask {
    pub id: String,              // UUID
    pub from: String,            // 委托人
    pub to: String,              // 承接人
    pub brief: String,           // 一句话简述
    pub detail: String,          // 详细描述
    pub parent_id: Option<String>, // 上级委托 ID（形成委托链）
    pub priority: u32,           // 优先级（用户消息 = u32::MAX）
    pub result: Option<String>,  // 执行结果
    pub reject_count: u32,       // 驳回次数（超阈值升级到 manager）
}
```

## 委托链

当成员 A 委托给 B，B 再委托给 C 时，形成链式关系。Margatroid 在两个方向上处理委托链：

### 向上追溯（执行任务时）

成员接手任务后，`build_chain()` 沿 `parent_id` 一路向上查找，收集每一级祖先的完整格式化输出，按祖先→子孙顺序叠放，作为上下文注入 LLM prompt。LLM 能完整理解"这条任务是怎么一路委派下来的"。

### 向下插入（审查返回时）

子委托完成并返回后，`review_delegations()` 将子委托的 `format()` 输出通过 `append_result()` 追加到父级委托的 result 字段。父级的 result 里累积了所有子委托的完整产出。

### format() 输出格式

```
[委托 {id}]
简述: {brief}
详情: {detail}
委托人: {from}
承接人: {to}
上级委托: {parent_id}
结果:
{result}
```

## 成员控制循环

每个成员运行一个独立的 tokio 任务，在 `member_loop()` 中交替执行两个独立过程：

1. **review_delegations** — 检查自己发出去的委托是否有返回，调用 LLM 审核，通过则 accept，不通过则 reject
2. **execute_task** — 从发布区 claim 任务，构建委托链上下文，调用 LLM 执行，return 结果

两个过程不共享数据，互不干扰。

## Tool-Call 循环约束

`chat()` 中 LLM 必须通过工具调用才能退出循环：

- LLM 返回 bare text（无 tool_calls）→ 追加"你必须返回当前委托或发布新委托才能结束"，继续循环
- 调用 `finish` 工具 → 返回当前委托结果，退出循环
- 调用 `delegate` 工具 → 发布新委托，退出循环
- 其他工具（bash、recall、delegate_reject、schedule_*）→ 执行后继续循环

## 基本工具集

所有成员共享的基础工具：

| 工具 | 说明 |
|------|------|
| `bash` | 在沙箱中执行 shell 命令 |
| `delegate` | 委托子任务给其他成员（target、task、priority） |
| `delegate_reject` | 驳回收到的委托结果 |
| `recall` | 搜索工作日志和个人记忆 |
| `finish` | 完成当前委托并返回结果 |

Manager 额外拥有 `schedule_add`、`schedule_list`、`schedule_pop`、`schedule_remove`。

## 记忆系统（SQLite）

三张核心表：

### worklog（团队工作日志）
- 任务被 accept 时写入
- 记录谁（agent_id）、委托给谁（to_agent）、做了什么（summary）
- 通过 delegation_id 与 personal_memory 关联

### personal_memory（个人记忆）
- 任务 return 时写入
- 记录谁委托的（from_agent）、做了什么（summary）、标签（tags）
- 通过 delegation_id 与 worklog 关联

### delegations（委托持久化）
- 任务 offer 时写入
- 支持 `get_task(id)` 链向上查找
- 支持 `append_result(id, text)` 向下累积

### prepare_context 三阶段检索

执行任务时，`prepare_context()` 分三步注入上下文：

1. **近期工作日志** — `worklog.recent(20)`，团队最近做了什么
2. **个人相关记忆** — 通过自己 worklog 的 delegation_id 查 personal_memory
3. **关键词匹配** — 用任务描述搜索 worklog

## 项目结构

```
margatroid/
├── types/         # 共享类型定义（Identity, RequestTool, ChatRequest 等）
├── runtime/       # 核心运行时（Workspace, DelegationBoard, Member, memory）
│   ├── agent.rs   # Agent trait
│   ├── board.rs   # 四区委托板
│   ├── member.rs  # LLM 成员 + tool-call loop
│   ├── memory.rs  # SQLite 记忆系统
│   └── workspace.rs  # 团队容器 + 控制循环
├── compose/       # Compose 文件解析
├── assets/        # 成员库（member.toml + SOUL.md）
├── sandbox/       # 沙箱执行环境
├── providers/     # LLM 供应商适配（OpenRouter 等）
├── cli/           # 命令行入口
└── members/       # 示例成员（eulgo, manager, coder, reviewer）
```

## Compose 文件

compose 文件只引用成员 ID，不定义成员本身：

```toml
[workspace]
name = "demo"
version = "0.1"

[[agents]]
id = "manager"

[[agents]]
id = "coder"

[[agents]]
id = "reviewer"
```

成员的实际定义（身份、模型、系统提示词）在 `~/.margatroid/members/{id}/member.toml` + `SOUL.md` 中。
