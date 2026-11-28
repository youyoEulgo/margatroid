# Agent

Agent 是 Workspace 中承载 MCL、工具任务、推理和持久会话的运行实体。
Lua 任务通过创建请求得到 `AgentHandle`，等待 ready 后开展交互；
创建任务通过 child 绑定监督 Agent 的关闭。

## 核心概念

- `Agent` 保存所属 Workspace、实际父实体与任务、实例标识、项目目录、状态和 turn。
- `AgentState` 包含 `Starting`、`Running`、`Closing`；创建完成处于 Starting，ready 后进入 Running。
- turn 是 Running 状态中的回合标识，开始返回当前或新建标识，结束和中止清除关联。
- `AgentHandle` 提供实体标识、ready 等待与异步关闭能力。
- `AgentUpdateContext` 将实体变化、公共工具/模型/沙箱信息和来源生命周期交给装配层观察。

## 一次创建与关闭

调用方先准备运行中的 Workspace、父实体上的实际工具任务与已打开的 Agent 存储，
再提交 `StartAgent<AgentStorage>`。请求携带父地址下的 Agent 地址、
绝对项目目录和 UI 目录，以及接收结果的 oneshot。

AgentPlugin 在调度帧中校验这些条件并创建实体，安装 MCL 队列、工具集合、
工具任务、推理状态和存储，再把 handle 绑定到创建任务。
调用方获得 handle 后，通过 ready 流程进入 Running，随后使用 MCL 开始回合。

关闭可以从 `AgentHandle::close()` 发起，也可提交按地址定位的 `StopAgent`。
前者关闭该 Agent；后者找到其创建任务并等待任务 finish，连同绑定的 Agent 一起收口。
关闭过程等待子任务、推理、来源观察、MCL 和存储清理，成功后移除实体。
完整创建夹具与两种关闭路径见 [生命周期测试](plugin/tests/lifecycle.rs)。

## 开发入口

| 位置 | 适用工作 |
| --- | --- |
| [agent_core](core/README.md) | 操作状态与 turn，接入 ready、handle 和更新观察 |
| [agent_plugin](plugin/README.md) | 安装生命周期系统，提交创建与关闭请求 |
| [Storage](../storage/README.md) | 准备 AgentStorage 与持久配置 |
| [LuaRuntime](../lua_runtime/README.md) | 从实际 Lua 任务创建和操作 Agent |
| [设计](DESIGN.md) | 查阅领域结构与调用关系 |

建议先运行 `cargo test -p agent_core -p agent_plugin`，
再阅读 [装配集成测试](../compose/tests/agent_creation_paths.rs) 中的真实创建路径。
