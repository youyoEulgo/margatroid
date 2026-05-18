# Margatroid 架构文档

## 概述

Margatroid 是一个用 Rust 实现的多 Agent 协作运行时，模拟人类团队的工作方式：通过委托（Delegation）机制分发任务，成员在沙箱中执行。当前阶段为单线程模型——同一时刻只有一个成员在执行委托。

核心设计理念是将委托过程建模为**任务链（TaskChain）**，类似图灵机的纸带：读写头在链上移动，委托时右移、完成时左移，上下文即整条链。

## Provider 隔离

runtime crate 完全不接触 provider 实现。`DynAiProvider` trait 和 `ProviderError` 定义在 `types` crate 中，`providers` crate 负责实现和解析：

```
types::DynAiProvider   ← runtime 持有，通过它做 LLM 调用
types::AiProvider      ← providers 实现，blanket impl → DynAiProvider
providers::resolve()   ← 按成员 ID 从配置自动查找并构建 provider
providers::build()     ← 按 provider_type 分发构造
```

## 任务链（TaskChain）

### 数据结构

```rust
enum ChainEntry {
    Delegate {
        id: String,          // 委托 ID
        from: String,        // 委托人
        to: String,          // 承接人
        brief: String,       // 简述
        detail: String,      // 详细描述
        parent_idx: usize,   // 父委托位置
    },
    Outcome {
        delegation_id: String, // 对应委托 ID
        content: String,       // 产出内容
        summary: String,       // 产出摘要
        done: bool,            // 委托是否完成
    },
}

struct TaskChain {
    entries: Vec<ChainEntry>,  // 链（只追加）
    head: usize,               // 读写头：当前活跃委托位置
}
```

### 操作

- **delegate（右移）**：`entries.push(Delegate { parent_idx: head, ... })` → `head = entries.len() - 1`
- **finish（左移）**：`entries.push(Outcome { delegation_id, done })` → 如果完成则 `head = entries[head].parent_idx`
- **上下文**：`entries[0..]` 就是从根到当前的全部记录，直接转为 LLM 上下文

### 多点 Outcome

一个委托可以产出多个 Outcome，配合子委托形成工作流：

```
Delegate(d1, manager→A)          head=0
Outcome(d1, "初稿", done:false)   head=0  ← A 发现自己需要帮助
Delegate(d2, A→B)                head=2  ← B 接手
Outcome(d2, "安全审计通过", true)  head=0  ← 回到 A
Outcome(d1, "修订", done:false)   head=0  ← A 再次需要帮助
Delegate(d3, A→C)                head=5  ← C 接手
Outcome(d3, "性能测试通过", true)  head=0  ← 回到 A
Outcome(d1, "最终版", done:true)  head=0  ← 完成，可返回上级
```

查询委托是否完成：找到该委托的最后一个 Outcome，看 `done` 字段。

### 与委托板的关系

`DelegationBoard` 作为持久化存储层保留。`Delegate` 条目通过 `id` 从 board 借用数据，不持有拷贝。`Outcome` 是链独有的运行时产物，不存 board。

未来多线程调度和可视化都需要委托板提供的全局视图。

## 成员（Member）

每个成员封装了 LLM 模型、系统提示词（SOUL.md）、沙箱访问和工具集。成员不区分身份——User、Manager、Member 三种 Identity 仅在成员库中标记。

成员通过 `Agent` trait 暴露统一接口。对应的 Provider 通过 `providers::resolve()` 按成员 ID 自动注入。

## Prompt 结构

```rust
struct Prompt {
    messages: Vec<ChatMessage>,
}
```

`Prompt::build(soul, task_chain, worklog, memories)` 组装上下文消息列表——不拼接模板，不关心格式，对外只暴露消息数组。Member 直接馈入 tool-call 循环。

## Tool-Call 循环约束

`chat()` 中 LLM 必须通过工具调用才能退出循环：

- LLM 返回 bare text → 追加"你必须返回当前委托或发布新委托才能结束"，继续循环
- 调用 `finish` → 返回当前委托结果（可能未完成），退出循环
- 调用 `delegate` → 发布新委托，退出循环
- 其他工具（bash、recall、schedule_*）→ 执行后继续循环

## 基本工具集

| 工具 | 说明 |
|------|------|
| `bash` | 在沙箱中执行 shell 命令 |
| `delegate` | 委托子任务给其他成员（target、task、detail、priority） |
| `recall` | 搜索工作日志和个人记忆 |
| `finish` | 完成或阶段性产出当前委托结果 |

Manager 额外拥有 `schedule_add`、`schedule_list`、`schedule_pop`、`schedule_remove`。

## 记忆系统（SQLite）

### worklog（团队工作日志）
- 委托最终完成时写入
- 记录谁（agent_id）、委托给谁（to_agent）、做了什么（summary）

### personal_memory（个人记忆）
- 委托产出时写入
- 记录谁委托的（from_agent）、做了什么（summary）、标签（tags）

### delegations（委托持久化）
- 委托创建时写入
- 支持链上查找

## 项目结构

```
margatroid/
├── types/         # 共享类型定义（DynAiProvider, ProviderError, Identity, ChatRequest 等）
│   └── provider.rs  # DynAiProvider + AiProvider trait + blanket impl
├── runtime/       # 核心运行时（Workspace, DelegationBoard, Member, TaskChain, memory）
├── providers/     # LLM 供应商（OpenRouter）+ resolve() / build()
├── compose/       # Compose 文件解析
├── assets/        # 成员库（member.toml + SOUL.md）
├── sandbox/       # 沙箱执行环境
├── cli/           # 命令行入口
└── server/        # HTTP API（factory 委托 providers::build()）
```

## Compose 文件

```toml
[workspace]
name = "demo"
version = "0.1"

[[agents]]
id = "manager"

[[agents]]
id = "coder"
```

成员定义在 `~/.margatroid/members/{id}/member.toml` + `SOUL.md`。
