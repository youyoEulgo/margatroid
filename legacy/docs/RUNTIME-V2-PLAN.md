# Runtime 架构文档

## Kernel（内核）

Runtime 的唯一入口。持有 EventBus、共享配置、workspace 列表。server 和 cli 只跟 Kernel 交互。signature 见下方对象签名汇总。

## Workspace

一个 workspace 一份。包含所有运行时状态。signature 见下方对象签名汇总。

Workspace::start() 接收 `Arc<EventBus>`，自身不存该字段，但会传给每个 Member。

## DelegationBoard

委托板是纯调度层。职责只有委托链管理、发布区缓存、成员通知。
不碰事件、不碰提示词组装、不碰配置。

```rust
pub struct DelegationBoard {
    publish: RwLock<Vec<DelegationTask>>,
    chain: RwLock<TaskChain>,
    db: Arc<SqliteMemory>,
    member_ids: RwLock<HashSet<String>>,
    notifies: RwLock<HashMap<String, Arc<Notify>>>,
}

impl DelegationBoard {
    pub async fn offer(...) -> Result<String>;
    pub async fn result(...) -> Result<()>;
    pub async fn take(...) -> Option<DelegationTask>;
    pub async fn status(...) -> BoardStatus;
    pub async fn chain_snapshot(&self) -> TaskChain;
    pub fn db(&self) -> &SqliteMemory;
    pub async fn wait(&self, member_id: &str);
}
```

五个字段，对应五个核心能力。

`publish` 是发布区。`offer()` 追加，`result(done=true)` 移除。
`chain` 是委托链。append-only，head 右移/左移。链操作时通过 `db` 写入 worklog 和 memory。
`member_ids` 是成员 ID 集合。`offer()` 校验目标成员合法性。
`notifies` 存每个成员的唤醒信号。`offer()` 和 `result(done=true)` 在链头变动后唤醒目标成员。

迁出的内容：

| 迁出 | 去向 | 原因 |
|------|------|------|
| events + 三个方法 | Kernel.event_bus (EventBus) | 全局通道基础设施，非委托板职责 |
| system_prompt | Workspace | compose 配置，非委托板职责 |
| member_profiles | Workspace | 成员管理，非委托板职责 |
| cached_worklog | Workspace（或 context.rs） | 提示词缓存，非委托板职责 |
| assemble_prompt | context.rs | 提示词拼装，非委托板职责 |

资源清理策略：

- **per-task 通道**：`result(done=true)` 时调用 `event_bus.unregister("demo/task/abc-123")` 清理
- **notifies**：`Workspace::shutdown` 时统一清理 `board.notifies`（或在 member 退出时单独清理）
- **SQLite 并发**：SqliteMemory 使用 WAL 模式 + `busy_timeout(5000)`，支持多读一写，worklog 写入无需额外事务保护

## EventBus

事件通道注册表。所有 workspace 的广播通道集中管理，用命名前缀区分 workspace。

```rust
pub struct EventBus {
    channels: RwLock<HashMap<String, broadcast::Sender<String>>>,
}

impl EventBus {
    pub fn register(&self, name: &str) -> broadcast::Receiver<String>;
    pub fn subscribe(&self, name: &str) -> Option<broadcast::Receiver<String>>;
    pub fn send(&self, name: &str, data: String) -> Result<usize>;
    pub fn unregister(&self, name: &str) -> bool;
}
```

四个方法。

`register` 创建 `broadcast::channel(32)`，存 tx，返回 rx。workspace 启动时调，per-task 通道创建时也调。
`subscribe` 在已有通道上拿新 rx。每个前端 SSE 连接调一次。
`send` 取出 tx 发消息，返回接收者数量。如果通道不存在返回 Err，如果发送失败（通道满/无接收者）记录警告并返回 Ok(0)。
`unregister` 移除通道，返回是否存在。任务完成时清理 per-task 通道。

命名按 `<workspace>/<用途>` 分层。

```
"demo/stream"           workspace 统一事件流
"demo/task/abc-123"     per-task 通道
"demo/task/def-456"     per-task 通道
"staging/stream"        另一个 workspace
```

Kernel 在启动时创建 EventBus，`Arc` 包一层。Workspace::start() 接收 `Arc<EventBus>` 引用，
传给 member_loop 供成员发事件。server handler 通过 `kernel.event_bus.subscribe()` 订阅通道。

最终 runtime 八个文件。

| 文件 | 职责 |
|------|------|
| `kernel.rs` | Kernel struct，workspace 创建/销毁/查询 |
| `workspace.rs` | Workspace struct，boot + 关停，成员循环，任务分发，工具定义，路径辅助 |
| `board.rs` | DelegationBoard + TaskChain，offer/result/take/status/wait/notify |
| `events.rs` | EventBus，channels + 构造 WorkspaceEvent + send/subscribe |
| `context.rs` | assemble_prompt + format_worklog |
| `member.rs` | Agent trait + Member + chat() + execute_* + 纯函数 |
| `memory.rs` | SqliteMemory |
| `client.rs` | Client，封装 model + provider + 流式/降级 |

## 对象归属

### 每个 workspace 一个

| 对象 | 存放处 | 生命周期 |
|------|--------|----------|
| DelegationBoard | Workspace.board | 随 workspace 创建/销毁 |
| SqliteMemory | Workspace.db | 随 workspace 创建/销毁 |
| SandboxManager | Workspace.sandbox | 随 workspace 创建/销毁 |
| 成员列表 | Workspace.members | 随 workspace 创建/销毁 |
| CancellationToken | Workspace.shutdown | 随 workspace 创建/销毁 |
| 成员循环句柄 | Workspace.handles | 随 workspace 创建/销毁 |

### 全 Kernel 共享

| 对象 | 存放处 | 生命周期 |
|------|--------|----------|
| 配置管理 | Kernel.config_mgr | 进程启动到退出 |
| EventBus | Kernel.event_bus | 进程启动到退出 |
| Provider 工厂 | Kernel 内部（从 config_mgr 读） | 进程启动到退出 |
| 成员库 | Kernel 内部（从 config_mgr 读） | 进程启动到退出 |

## 对象签名汇总

所有 struct 的完整字段和关键方法。

### Kernel

```rust
pub struct Kernel {
    pub event_bus: Arc<EventBus>,
    pub config_mgr: assets::Manager,
    workspaces: RwLock<HashMap<String, Arc<Workspace>>>,
}

impl Kernel {
    pub fn new(config_mgr: assets::Manager) -> Self;
    pub async fn create_workspace(&self, name: &str, compose: &ComposeFile, entries: Vec<AgentEntry>) -> Result<Arc<Workspace>>;
    pub fn workspace(&self, name: &str) -> Option<Arc<Workspace>>;
    pub async fn remove_workspace(&self, name: &str) -> Option<Arc<Workspace>>;
    pub fn list_workspaces(&self) -> Vec<String>;
    pub async fn shutdown_all(&self);
}
```

### MemberProfile

```rust
pub struct MemberProfile {
    pub id: String,
    pub display_name: String,
    pub tags: Vec<String>,
}
```

### Workspace

```rust
pub struct Workspace {
    pub name: String,
    pub board: Arc<DelegationBoard>,
    pub sandbox: Arc<RwLock<SandboxManager>>,
    pub db: Arc<SqliteMemory>,
    pub system_prompt: String,
    pub member_profiles: Vec<MemberProfile>,

    members: HashMap<String, Arc<dyn Agent>>,
    handles: Vec<JoinHandle<()>>,
    shutdown: CancellationToken,
}

impl Workspace {
    pub async fn start(
        name: String,
        compose: &ComposeFile,
        entries: Vec<AgentEntry>,
        event_bus: Arc<EventBus>,
    ) -> Result<Self>;
    pub async fn send_user_message(&self, from: &str, to: &str, brief: &str, detail: &str) -> Result<String>;
    pub async fn shutdown(self);
}
```

### DelegationBoard

```rust
pub struct DelegationBoard {
    publish: RwLock<Vec<DelegationTask>>,
    chain: RwLock<TaskChain>,
    db: Arc<SqliteMemory>,
    member_ids: RwLock<HashSet<String>>,
    notifies: RwLock<HashMap<String, Arc<Notify>>>,
}

impl DelegationBoard {
    pub async fn offer(&self, from: &str, to: &str, brief: &str, detail: &str, parent_id: Option<&str>) -> Result<String>;
    pub async fn result(&self, member_id: &str, result: TaskResult) -> Result<()>;
    pub async fn take(&self, member_id: &str) -> Option<DelegationTask>;
    pub async fn status(&self) -> BoardStatus;
    pub async fn chain_snapshot(&self) -> TaskChain;
    pub fn db(&self) -> &SqliteMemory;
    pub async fn wait(&self, member_id: &str);
}
```

### EventBus

```rust
pub struct EventBus {
    channels: RwLock<HashMap<String, broadcast::Sender<String>>>,
}

impl EventBus {
    pub fn register(&self, name: &str) -> broadcast::Receiver<String>;
    pub fn subscribe(&self, name: &str) -> Option<broadcast::Receiver<String>>;
    pub fn send(&self, name: &str, data: String) -> Result<usize>;
    pub fn unregister(&self, name: &str) -> bool;
}
```

### Member

```rust
pub struct Member {
    pub id: String,
    soul: String,
    identity: Identity,
    client: Client,
    sandbox: Arc<RwLock<SandboxManager>>,
    event_bus: Arc<EventBus>,
}

impl Member {
    pub fn new(
        id: &str,
        soul: String,
        identity: Identity,
        client: Client,
        sandbox: Arc<RwLock<SandboxManager>>,
        event_bus: Arc<EventBus>,
    ) -> Self;
}

#[async_trait::async_trait]
impl Agent for Member {
    fn id(&self) -> &str;
    fn identity(&self) -> &Identity;
    async fn process(&self, board: &DelegationBoard, tools: &[RequestTool]) -> Result<ChatOutcome>;
}
```

### AgentEntry（传给 Workspace::start 的参数）

```rust
pub struct AgentEntry {
    pub agent: Arc<dyn Agent>,
    pub soul: String,
    pub tools: Vec<RequestTool>,
    pub skills: Vec<String>,
}
```

### 持有链

```
Kernel
  └── event_bus: Arc<EventBus>
  └── config_mgr: Manager
  └── workspaces: HashMap<String, Arc<Workspace>>

Workspace
  └── board: Arc<DelegationBoard>
  └── sandbox: Arc<RwLock<SandboxManager>>
  └── db: Arc<SqliteMemory>
  └── members: HashMap<String, Arc<dyn Agent>>
        └── Member
              └── event_bus: Arc<EventBus>  ── clone 自 Kernel.event_bus, 用于 chat() 发事件
              └── client: Client
              └── sandbox: Arc<RwLock<SandboxManager>>  ── clone 自 Workspace.sandbox

DelegationBoard
  └── db: Arc<SqliteMemory>  ── clone 自 Workspace.db
```

```
cli / server
  └── Kernel::new(config_mgr)
        ├── 创建 EventBus（全局通道注册表）
        └── Kernel::create_workspace("demo", compose, entries)
              └── Workspace::start(name, compose, entries, event_bus.clone())
                    ├── event_bus.register("demo/stream")    // 注册统一事件流通道
                    ├── 创建 DelegationBoard
                    │     ├── TaskChain(虚拟根)
                    │     ├── 发布区
                    │     └── notify_member 机制
                    ├── 创建 SqliteMemory(memory.db)
                    ├── 创建 SandboxManager
                    ├── 遍历 AgentEntry → 创建 Member
                    │     └── member_loop 接收 Arc<EventBus> clone
                    └── 返回 Workspace

运行中:
  member_loop → execute_task → chat() → LLM stream chunk
    ├── 每 chunk: 构造 WorkspaceEvent → event_bus.send("demo/stream", json)
    └── delegate/finish: board.offer/result
          └── 外部调: event_bus.send("demo/stream", chain_update/board_update)

  offer() 时:
    event_bus.register("demo/task/abc-123")  // 创建 per-task 通道

server SSE:
  GET /workspace/{name}/stream
    └── kernel.event_bus.subscribe("demo/stream")
          └── BroadcastStream → axum Sse → 前端 EventSource
```

## 事件流

Kernel 持有 EventBus，workspace 不持有。所有通道操作走 Kernel。

发射点：

| 事件类型 | 谁发 | 经过 |
|----------|------|------|
| board_update | Workspace（外部调） | `kernel.event_bus.send("demo/stream", json)` |
| chain_update | Workspace（外部调） | `kernel.event_bus.send("demo/stream", json)` |
| member_status | Workspace | `kernel.event_bus.send("demo/stream", json)` |
| stream_chunk | Member.chat() | `kernel.event_bus.send("demo/stream", json)` |
| human_request | server human.rs | `kernel.event_bus.send("demo/stream", json)` |

所有走 `kernel.event_bus.send("demo/stream", ...)`，不经过中转。

## server 层改动

state.rs：

```rust
pub struct AppState {
    kernel: Arc<Kernel>,
    pending: ...,
}
```

不再直接访问 workspace map：

```rust
// 旧
state.workspaces.read().await.get("demo")
// 新
state.kernel.workspace("demo")
```

handler 里：

```rust
let ws = state.kernel.workspace(&name)?;
ws.board.status()
ws.db.recent(20)
state.kernel.event_bus.subscribe("demo/stream")
ws.send_user_message("user", "manager", "test", "")
```

## 执行计划

按顺序执行，每步保持可编译状态：

### ✅ 步骤 1：新建基础设施（已完成）

- ✅ 新建 `runtime_v2/src/events.rs` — EventBus struct + register/subscribe/send/unregister 四个方法
- ✅ 新建 `runtime_v2/src/kernel.rs` — Kernel struct，持有 event_bus + config_mgr + workspaces
- ✅ 在 `types/src/member.rs` 新增 `MemberProfile` struct

### ✅ 步骤 2：核心组件实现（已完成）

- ✅ 新建 `runtime_v2/src/board.rs` — DelegationBoard V2（纯调度层）
- ✅ 新建 `runtime_v2/src/context.rs` — 提示词组装逻辑
- ✅ 新建 `runtime_v2/src/member.rs` — Member 持有 EventBus，chat() 直接发送事件
- ✅ 新建 `runtime_v2/src/workspace.rs` — Workspace 接收 EventBus，管理成员和生命周期
- ✅ 新建 `runtime_v2/src/tools.rs` — 工具执行逻辑（bash, delegate, finish, recall, schedule_*）
- ✅ 新建 `runtime_v2/src/memory.rs` — SqliteMemory stub（临时实现）

**测试结果：20 个测试全部通过，约 1906 行代码**

### ✅ 步骤 3：完整的 SqliteMemory（已完成）

- ✅ 从旧 runtime 复制完整的 SqliteMemory 实现（742 行）
- ✅ 修复生命周期问题（personal_by_delegations 方法）
- ✅ 更新所有调用点（SqliteMemory::new → SqliteMemory::open）
- ✅ 验证测试通过（新增 4 个 memory 测试）

**测试结果：24 个测试全部通过**

### ⏳ 步骤 4：集成到 Kernel（待完成）

- [ ] Kernel::new 创建 EventBus
- [ ] Kernel::create_workspace 调用 Workspace::new，传入 event_bus.clone()
- [ ] Kernel::remove_workspace 调用 Workspace::shutdown + 清理 notifies
- [ ] 实现成员循环（member_loop）

### ⏳ 步骤 5：Server 侧改写（待完成）

- [ ] `state.rs`：AppState 持有 `Arc<Kernel>`
- [ ] `handlers/workspace.rs`：
  - 从 `state.kernel.workspace("demo")` 获取 workspace
  - 从 `state.kernel.event_bus.subscribe("demo/stream")` 订阅通道
  - board/chain 操作后手动调用 `event_bus.send()` 发送 board_update/chain_update
- [ ] `human.rs`：通过 kernel 获取 workspace 和 event_bus

### ⏳ 步骤 6：CLI 侧改写（待完成）

- [ ] `cmd_compose_up`：创建 Kernel，调用 kernel.create_workspace

### ⏳ 步骤 7：清理旧代码（待完成）

- [ ] 删掉 `types/src/events.rs` 中的 `BoardUpdateEvent` / `ChainUpdateEvent` / `MemberStatusEvent` / `HumanRequestEvent`（如果这些 struct 完全被 WorkspaceEvent 替代）
- [ ] 删掉 board.rs 中残留的事件相关导入

### ⏳ 步骤 8：编译、测试、验证（待完成）

- [ ] `cargo build`
- [ ] `cargo clippy -- -Dwarnings`
- [ ] `cargo test`
- [ ] 手动测试 SSE 流、任务委托、per-task 通道创建/清理
