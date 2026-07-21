# Margatroid V3 Plugin API 规范

状态：草案
目标：所有 V3 plugin 必须按本文档暴露 API、注册能力、声明边界。

## 设计目标

V3 plugin 的目标不是把功能拆成更多 crate，而是让每个功能边界都能被替换、测试和组合。

Plugin 必须满足：

- 可插拔：移除一个 plugin 不应破坏其他无直接依赖的 plugin。
- 可测试：同步 plugin 可以只和 `core_plugin` 一起测试；异步 plugin 只额外依赖明确声明的 async runtime plugin。
- 可组合：多个 plugin 通过事件和资源协作，而不是互相调用内部模块。
- 可替换：同类 plugin 通过相同事件和资源契约替换实现。
- 可观测：重要状态变化必须以事件形式暴露。

## 核心模型

所有 plugin 都围绕 `core_plugin` 工作：

```rust
pub trait Plugin {
    fn build(&self, app: &mut App);
}
```

`build` 是 plugin 唯一的安装入口。plugin 通过它注册：

- event
- resource
- system
- startup 初始化逻辑

异步能力不是 core API。需要异步工作的 plugin 通过 `async_runtime_plugin` 提供的扩展 API 注册任务。

Plugin 之间不直接持有彼此的具体类型。跨 plugin 通信优先使用事件，其次使用明确公开的 resource。

## Core API 规范（KISS）

`core_plugin` 是不可再拆的同步 ECS 内核，不是功能集合。判断一个能力是否应进入 core，只问一个问题：

> 删除该能力后，Entity、Component、Resource、System、Schedule、Event 或 Plugin 是否无法成立？

如果答案是否定的，该能力必须放入独立 plugin。

### Core 职责

core 只负责：

```text
App
├── World
│   ├── Entity / Component / Bundle
│   ├── Resource
│   └── Event / EventReader
├── Schedule
│   └── System / ordering / panic isolation
└── Plugin / PluginGroup
```

允许保留在 core 的行为：

- 创建、删除和验证 Entity
- 按类型存取 Component 和 Resource
- 注册并顺序执行同步 System
- 对命名 System 做 `before` / `after` 排序
- 隔离单个 System panic 并产生调度报告
- 注册、发送、独立读取和按帧清理 Event
- 安装 Plugin
- 执行单次同步 ECS 帧

### Core 禁止承担的职责

以下能力禁止进入 `core_plugin`：

- Tokio、async-std 或其他异步 runtime
- Future spawn、任务队列、完成通道
- timeout、cancel、retry、并发上限
- `AsyncTaskId`、异步任务状态和异步失败事件
- 阻塞运行循环、Condvar 唤醒和 shutdown 控制
- 网络、HTTP、SSE、WebSocket
- LLM provider、sandbox、skill、config
- workflow、agent、memory 等领域模型
- 某个业务 plugin 专用的 stage 或内部 hook

core 的依赖中不得出现 Tokio。可选基础设施不能通过 feature flag 塞回 core。

### Core 对象所有权

目标所有权必须保持为：

```text
App
├── World
├── HashMap<Stage, Schedule>
├── Event maintenance state
└── started / event retention 等少量帧状态

World
├── Component columns
├── Resource map
└── Entity allocator state

Schedule
└── Vec<Box<dyn System>>
```

`App` 不得持有 `AsyncWorker`、异步发送端、异步完成 Schedule 或异步配置。

### Core Stage

core 提供稳定的通用阶段：

```rust
pub enum Stage {
    Startup,  // 只运行一次
    First,    // 每帧最先运行，供基础设施 plugin 使用
    Update,   // 同步业务 System
    Last,     // 每帧最后运行，供基础设施 plugin 使用
}
```

固定顺序为：

```text
Startup（一次）
First → Update → Last
事件 frame 推进与过期清理
```

`First`、`Update` 和 `Last` 都没有 Margatroid 领域语义。core 不知道哪个 plugin 使用它们。
Input、Prepare、Execute、Finalize、Event 等业务顺序由 runtime 类 plugin 通过命名 System 和 ordering 约束表达，不进入 core enum。

### Core Public API 目标

core 的稳定公开 API 以以下形状为目标：

```rust
pub struct App;

impl App {
    pub fn new() -> Self;

    pub fn add_plugins(&mut self, plugins: impl Into<PluginGroup>) -> &mut Self;
    pub fn add_systems(
        &mut self,
        stage: Stage,
        systems: impl IntoIterator<Item = impl System>,
    ) -> &mut Self;

    pub fn add_resource<R: Resource>(&mut self, resource: R) -> &mut Self;
    pub fn add_event<E: Event>(&mut self) -> &mut Self;
    pub fn event_reader<E: Event>(&self) -> EventReader<E>;

    pub fn world(&self) -> &World;
    pub fn world_mut(&mut self) -> &mut World;
    pub fn schedule_mut(&mut self, stage: Stage) -> &mut Schedule;

    pub fn tick(&mut self);
}
```

允许存在少量直接服务上述 API 的配置方法，例如事件保留帧数。禁止在 `App` 上继续增加领域快捷方法。

core 的 `lib.rs` 只导出：

```text
App / Stage
World / Entity / Component / Bundle / Resource
Query / QueryMut / Res / ResMut
System / Schedule / ordering 与调度报告类型
Event / EventReader
Plugin / PluginGroup
```

core 不再导出任何 `Async*` 类型。

### CorePlugin 结论

删除 `CorePlugin` public 类型。`App::new()` 已经创建 ECS 内核，再安装一个空的 `CorePlugin` 没有实际行为，违反 KISS。

正确的组合方式是：

```rust
App::new()
    .add_plugins(AppRuntimePlugin::default())
    .add_plugins(AsyncRuntimePlugin::default())
    .add_plugins(LlmPlugin::default())
    .run();
```

不需要异步能力时：

```rust
App::new()
    .add_plugins(MySynchronousPlugin)
    .tick();
```

### Core 重构迁移顺序

本轮重构按以下顺序实施：

1. 新建 `async_runtime_plugin`，迁移 `async_runtime.rs` 和全部 `Async*` public 类型。
2. 将 core stage 收缩为 `Startup / First / Update / Last`，增加 `App::add_resource`。
3. 让 `AsyncRuntimePlugin` 注册 Startup、First、Last 三组内部 System。
4. 将 `add_async_system*` 从 `App` 固有方法迁移为 `AsyncAppExt`。
5. 从 `App` 删除 async worker、dispatch/completion Schedule 和异步配置字段。
6. 从 `core_plugin::lib.rs` 删除所有 `Async*` 导出和 `CorePlugin`。
7. 新建 `app_runtime_plugin`，迁移 `AppControl`、`run()`、阻塞等待和 shutdown。
8. 更新 LLM、sandbox、server 等 plugin 的依赖与测试。
9. 删除 core 的 Tokio 与运行循环依赖，并运行完整 workspace 测试与 clippy。

完成标准：

- `cargo tree -p core_plugin` 中不存在 Tokio
- `rg "Async|Future|tokio|mpsc" crates/core_plugin/src` 不命中异步实现
- `App` 不持有任何 worker 或 channel
- core 不含 Condvar、shutdown flag 或无限运行循环
- 纯同步 ECS 测试只依赖 `core_plugin`
- 异步集成测试显式安装 `AsyncRuntimePlugin`
- 移除 `AsyncRuntimePlugin` 后，同步 Plugin 仍可正常运行

## Crate 命名

官方 plugin crate 统一放在：

```text
margatroid/crates/
  core_plugin/
  app_runtime_plugin/
  async_runtime_plugin/
  config_plugin/
  event_bus_plugin/
  llm_plugin/
  sandbox_plugin/
  skill_plugin/
  server_plugin/
```

命名规则：

- crate 名使用 snake_case：`llm_plugin`
- plugin 类型使用 PascalCase：`LlmPlugin`
- 事件类型使用领域前缀：`LlmRequest`、`LlmResponse`
- resource 类型使用领域前缀：`LlmProviders`、`SkillRegistry`
- system 函数使用 snake_case：`dispatch_llm_requests`

## 标准目录结构

每个 plugin crate 默认使用：

```text
src/
  lib.rs
  plugin.rs
  events.rs
  resource.rs
  systems.rs
```

复杂 plugin 可以继续拆分：

```text
src/
  systems/
    dispatch.rs
    collect.rs
    maintenance.rs
```

但 `lib.rs` 仍然只导出稳定 API，不暴露内部系统细节。

## Public API 规则

`lib.rs` 只能导出：

- plugin 类型
- public events
- public resources
- public config/options
- 必要的 trait 或 handle 类型

示例：

```rust
mod events;
mod plugin;
mod resource;
mod systems;

pub use events::{LlmFailed, LlmRequest, LlmResponse, LlmStreamChunk};
pub use plugin::LlmPlugin;
pub use resource::{LlmProviderRegistry, LlmPluginOptions};
```

默认不导出：

- system 函数
- 内部 helper
- storage 细节
- channel 细节
- async worker 内部类型

## Plugin 构造 API

每个 plugin 必须提供一个零配置构造方式：

```rust
impl Default for LlmPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl LlmPlugin {
    pub fn new() -> Self {
        Self { options: Default::default() }
    }
}
```

需要配置时使用 builder 风格：

```rust
App::new()
    .add_plugins(
        LlmPlugin::new()
            .with_default_provider("openrouter")
            .with_timeout(Duration::from_secs(60)),
    );
```

Plugin 构造函数不能启动线程、打开 socket、读取配置文件或访问网络。这些动作只能在 `build` 注册的 system 中发生，或由对应 plugin 的明确 startup system 发生。

## build 规则

`Plugin::build(&self, app: &mut App)` 可以做：

- `app.add_event::<E>()`
- `app.add_resource(resource)`
- `app.add_systems(stage, systems)`
- 创建 event reader 并移动到 system 闭包中
- 使用已声明依赖的 plugin extension API，例如 `AsyncAppExt::add_async_system`

`build` 不应做：

- 阻塞 I/O
- 网络请求
- 长时间计算
- 启动不受 `App` 生命周期管理的后台线程
- 读取其他 plugin 的私有模块
- 修改全局进程状态，除非 plugin 职责明确要求

如果 plugin 需要初始化外部资源，应注册 `Stage::Startup` system。

## Stage 使用规范

当前标准 stage：

```text
Startup   初始化，只运行一次
First     每帧最先执行的通用基础设施挂载点
Update    所有同步业务 System
Last      每帧最后执行的通用基础设施挂载点
```

约定：

- 普通业务 plugin 只注册 `Startup` 和 `Update`
- `First` / `Last` 只供基础设施 plugin 使用，业务 plugin 不应赋予它们新的领域语义
- Input、Prepare、Execute、Finalize、Event 等领域顺序由未来 runtime 类 plugin 定义为稳定 system label / set
- 在领域调度 API 稳定前，plugin 通过 named system 的 `.before()` / `.after()` 表达顺序

Plugin 不应假设同 stage 内的注册顺序。需要顺序时必须使用 named system 和 `.before()` / `.after()`。

## 事件 API 规范

事件是 plugin 间的第一通信方式。

事件类型必须：

- `Clone + Send + Sync + 'static`
- 表示已经发生的事实，或明确的请求
- 不携带非托管引用
- 不暴露内部锁、channel、reader

推荐命名：

```text
XxxRequested   请求执行某动作
XxxStarted     动作已开始
XxxCompleted   动作成功完成
XxxFailed      动作失败
XxxEmitted     对外事件已产生
```

示例：

```rust
#[derive(Clone, Debug)]
pub struct LlmRequest {
    pub request_id: String,
    pub provider: String,
    pub model: String,
    pub messages: Vec<RequestMessage>,
    pub stream: bool,
}

#[derive(Clone, Debug)]
pub struct LlmResponse {
    pub request_id: String,
    pub response: ChatResponse,
}

#[derive(Clone, Debug)]
pub struct LlmFailed {
    pub request_id: String,
    pub error: String,
}
```

同一个 plugin 消费自己发出的事件是允许的，但必须用独立 `EventReader`，不能假设事件只被读取一次。

## Resource API 规范

Resource 用于保存 World 级共享状态。

Resource 必须：

- `Send + Sync + 'static`
- 对外暴露稳定方法，不暴露内部容器
- 避免让调用者持有长期锁
- 避免让外部直接修改破坏不变量的字段

推荐形式：

```rust
pub struct SkillRegistry {
    inner: RwLock<HashMap<String, SkillDescriptor>>,
}

impl SkillRegistry {
    pub fn register(&self, skill: SkillDescriptor) -> Result<()>;
    pub fn get(&self, name: &str) -> Option<SkillDescriptor>;
    pub fn list(&self) -> Vec<SkillDescriptor>;
}
```

Resource 可以提供 handle，但 handle 必须清楚说明生命周期和线程安全语义。

## Async API 规范

所有 ECS system 必须是同步函数。

异步能力由独立的 `async_runtime_plugin` 提供，`core_plugin` 不包含 Future、worker、channel 或 task 类型。

应用必须先显式安装 runtime：

```rust
use async_runtime_plugin::AsyncRuntimePlugin;

let mut app = App::new();
app.add_plugins(AsyncRuntimePlugin::default());
```

需要注册异步任务的 plugin 通过扩展 trait 使用 API：

```rust
use async_runtime_plugin::{AsyncAppExt, AsyncSystemOptions};

app.add_async_system_with_options(
    |request: LlmRequest| async move {
        // async I/O
        LlmResponse { /* ... */ }
    },
    AsyncSystemOptions::default(),
);
```

扩展 API 目标：

```rust
pub trait AsyncAppExt {
    fn add_async_system<Request, Output, Handler, Fut>(
        &mut self,
        handler: Handler,
    ) -> &mut Self;

    fn add_async_system_with_options<Request, Output, Handler, Fut>(
        &mut self,
        handler: Handler,
        options: AsyncSystemOptions,
    ) -> &mut Self;
}
```

调用扩展 API 前必须已经安装 `AsyncRuntimePlugin`。缺失时在 plugin build 阶段立即给出明确错误，不允许静默降级或隐式启动另一套 runtime。

内部数据流固定为：

```text
Request event
  -> Stage::Last 的 AsyncDispatch System
  -> bounded task channel
  -> async worker
  -> completion channel
  -> 下一帧 Stage::First 的 AsyncCompletion System
  -> Output event / AsyncTaskFailed
```

业务 Plugin 不应自己创建 Tokio runtime。`async_runtime_plugin` 同时承担稳定异步注册接口和默认实现；未来替换执行后端时，应在该 crate 内通过明确的 executor backend trait 扩展，不能把执行器接口塞回 core。

异步 request/output 必须是不同事件类型。

异步任务必须满足：

- 每个 request 单独 spawn
- 阻塞操作必须使用 `tokio::task::spawn_blocking`
- 失败必须转成领域失败事件
- 超时、取消、panic 需要能被观测
- 不把 provider API key 写进事件、日志或错误详情

## Error API 规范

跨 plugin 的失败必须以事件表达。

推荐：

```rust
#[derive(Clone, Debug)]
pub struct SandboxCommandFailed {
    pub command_id: String,
    pub kind: SandboxFailureKind,
    pub message: String,
}

#[derive(Clone, Debug)]
pub enum SandboxFailureKind {
    PermissionDenied,
    Timeout,
    ExitNonZero,
    SpawnFailed,
    Cancelled,
}
```

错误事件中不放：

- API key
- token
- 原始 Authorization header
- 完整用户隐私文件内容
- 不必要的大块 stdout/stderr

## Plugin 依赖规范

Plugin 依赖分三类：

```text
Required dependency
  缺失时 plugin 无法工作。若 build 阶段即可判断，应立即报错；否则在 Startup 失败并发出错误事件。

Optional dependency
  缺失时禁用部分功能，但 plugin 仍可启动。

Event dependency
  只依赖某些事件存在，不依赖对方具体 crate 内部。
```

避免：

```text
server_plugin -> runtime_plugin -> llm_plugin -> server_plugin
```

推荐：

```text
server_plugin -> core_plugin
server_plugin -> app_runtime_plugin API
server_plugin -> event_bus_plugin API
llm_plugin    -> core_plugin + async_runtime_plugin + providers + types
sandbox_plugin -> core_plugin + async_runtime_plugin
workflow_plugin 通过事件使用 llm_plugin
```

PluginGroup 按添加顺序调用 `build`。基础设施 plugin 必须先于依赖它的业务 plugin 安装：

```rust
App::new()
    .add_plugins(AppRuntimePlugin::default())
    .add_plugins(AsyncRuntimePlugin::default())
    .add_plugins(LlmPlugin::default());
```

这个顺序只决定安装依赖，不决定同一 Stage 内的 System 执行顺序；System 顺序仍由 `before` / `after` 明确声明。

## 基础设施 Plugin 契约

### AppRuntimePlugin

职责：

- 提供阻塞式 `run()` 扩展 API
- 在没有工作时阻塞当前线程，避免忙轮询
- 提供可克隆的 wake / shutdown handle
- 收到 wake 后调用下一次 `App::tick()`
- 管理主循环开始、停止和退出状态

公开 API：

```text
AppRuntimePlugin
AppRunExt
AppControl
```

不负责：

- 保存 ECS 数据
- 定义业务 Stage
- 创建异步任务 worker
- 解释某次 wake 的业务原因

最小组合：

```rust
use app_runtime_plugin::{AppRunExt, AppRuntimePlugin};

let mut app = App::new();
app.add_plugins(AppRuntimePlugin::default());
app.run();
```

`AppControl` 作为 Resource 由 World 持有。任何需要唤醒主循环的 plugin 都只能依赖该公开 handle，不能接触 Condvar 或内部运行状态。

### AsyncRuntimePlugin

职责：

- 提供 `AsyncAppExt`
- 管理异步 runtime 和工作线程生命周期
- 将 Request event 派发为独立 Future
- 将完成结果送回主线程并转换为 Output event
- 提供队列容量、并发上限、超时和取消
- 异步任务完成时，如果存在 `AppControl` Resource，则唤醒主循环

注册位置：

```text
Stage::Startup  启动受 App 生命周期管理的 worker
Stage::First    回收 Completion 并写回 World
Stage::Last     读取 Request event 并派发任务
```

公开 API：

```text
AsyncRuntimePlugin
AsyncRuntimeOptions
AsyncAppExt
AsyncSystemOptions
AsyncTaskId
AsyncTaskControl
AsyncTaskStarted
AsyncTaskFailed
AsyncTaskFailureKind
```

内部 API：

```text
AsyncWorker
AsyncSpawner
AsyncTask
Completion
WorldCommand
所有 channel sender / receiver
```

不负责：

- 定义 LLM、sandbox、skill 等领域 Request / Output
- 决定业务任务何时产生
- 让 Future 直接借用或修改 World
- 为缺失的业务失败事件提供 fallback

生命周期约束：

- Plugin 构造函数和 `build` 不启动线程
- `build` 只注册异步 handler 和未启动的 runtime 状态
- worker 在 `Stage::Startup` 启动
- worker 启动 System 使用稳定 label `async_runtime.start`
- worker handle 作为 Resource 由 World 持有
- 与 `AppRuntimePlugin` 组合时，完成通道通过 `AppControl` 唤醒阻塞主循环
- 不安装 `AppRuntimePlugin` 时允许测试代码手动调用 `App::tick()` 回收结果
- Resource Drop 时发送 Shutdown、取消剩余任务并 join 线程
- 每个 Request 单独 spawn
- 队列必须有界，并发数必须有上限

### ConfigPlugin

职责：

- 管理配置路径
- 加载 daemon/workspace/provider 配置
- 发出配置加载和变更事件

公开事件：

```text
ConfigLoadRequested
ConfigLoaded
ConfigReloaded
ConfigLoadFailed
```

公开资源：

```text
ConfigStore
```

不负责：

- 创建 workspace
- 构造 provider
- 启动 server

### EventBusPlugin

职责：

- 管理命名广播通道
- 接收 `WorkspaceEventEmitted`
- 提供外部订阅接口给 server/CLI

公开事件：

```text
WorkspaceEventEmitted
EventBusPublishFailed
```

公开资源：

```text
EventBus
```

不负责：

- 定义 runtime 任务状态
- 生成 LLM chunk
- 持久化 memory

### LlmPlugin

职责：

- 注册 provider
- 消费 `LlmRequest`
- 调用 provider async API
- 发出 `LlmResponse`、`LlmStreamChunk`、`LlmFailed`

公开事件：

```text
LlmRequest
LlmResponse
LlmStreamChunk
LlmFailed
```

公开资源：

```text
LlmProviderRegistry
```

不负责：

- 决定哪个 agent 应该说话
- 执行 workflow
- 写 memory
- 发送 SSE

### SandboxPlugin

职责：

- 执行命令或沙箱任务
- 做权限和隔离策略
- 返回 stdout/stderr/exit status

公开事件：

```text
SandboxCommandRequested
SandboxCommandStarted
SandboxCommandCompleted
SandboxCommandFailed
```

公开资源：

```text
SandboxPolicy
SandboxExecutor
```

不负责：

- 解释工具语义
- 判断 agent 是否应该运行命令
- 直接读取 LLM 输出

### SkillPlugin

职责：

- 扫描 skill 文件
- 解析 frontmatter
- 分类 member skill / workflow skill
- 管理 skill registry
- 处理 skill load/unload 请求

公开事件：

```text
SkillScanRequested
SkillScanned
SkillScanFailed
SkillLoadRequested
SkillLoaded
SkillLoadFailed
SkillUnloadRequested
SkillUnloaded
```

公开资源：

```text
SkillRegistry
LoadedSkills
```

不负责：

- 执行 workflow DAG
- 调用 LLM
- 执行 sandbox command

### ServerPlugin

职责：

- 启动 daemon HTTP API
- 将 HTTP/CLI/Web 输入转为 App 事件
- 将 EventBus 订阅转为 SSE/WebSocket 响应

公开事件：

```text
HttpRequestReceived
UserPromptSubmitted
ServerStarted
ServerFailed
ShutdownRequested
```

公开资源：

```text
ServerHandle
ServerConfig
```

不负责：

- 业务调度
- LLM provider 调用
- workflow 执行
- memory 写入

## Runtime 类 Plugin 延后规范

以下 plugin 暂不稳定，等基础设施稳定后再定：

```text
workspace_plugin
workflow_plugin
member_plugin
memory_plugin
```

但它们将来仍必须遵守本文档的通用规则：

- system 同步
- async 通过 `async_runtime_plugin` 的事件 request/output 契约
- 跨 plugin 用事件
- 对外只导出稳定 API
- 不让 LLM 控制流程
- 不把领域 Stage 重新硬编码进 core

## 测试要求

每个 plugin 至少需要：

- resource 单元测试
- event chain 测试
- plugin build smoke test
- 失败事件测试

涉及 async 的 plugin 还需要：

- 成功路径
- 失败路径
- 超时路径
- 取消路径，如果支持取消

涉及外部服务的测试必须默认 ignored，并通过环境变量读取凭据。

禁止在测试代码中写入真实 API key。

## 文档要求

每个 plugin crate 根目录必须有：

```text
README.md
```

README 至少包含：

- plugin 职责
- public events
- public resources
- stage 注册说明
- 最小使用示例
- 不负责的边界

## 稳定性等级

Public API 分为三类：

```text
Stable
  lib.rs 公开导出的事件、资源、plugin 构造 API。

Internal
  systems.rs、内部 storage、helper。

Experimental
  明确标注的 runtime/workflow/member API。
```

破坏 Stable API 必须更新本文档，并在 commit message 中写明 Breaking changes。

## 最小 Plugin 示例

```rust
use core_plugin::{App, Plugin, Stage, World};

#[derive(Default)]
pub struct ExamplePlugin;

#[derive(Clone, Debug)]
pub struct ExampleRequested {
    pub id: String,
}

#[derive(Clone, Debug)]
pub struct ExampleCompleted {
    pub id: String,
}

impl Plugin for ExamplePlugin {
    fn build(&self, app: &mut App) {
        app.add_event::<ExampleRequested>();
        app.add_event::<ExampleCompleted>();

        let mut reader = app.event_reader::<ExampleRequested>();
        app.add_systems(Stage::Update, [move |world: &mut World| {
            for event in world.read_events(&mut reader) {
                world.send_event(ExampleCompleted { id: event.id });
            }
        }]);
    }
}
```

这个例子体现了 plugin 的基本约束：外部只知道事件，system 细节留在 plugin 内部。
