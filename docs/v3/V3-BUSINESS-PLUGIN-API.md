# V3 业务 Plugin API 规范

状态：草案

目标：定义 Margatroid 领域 Plugin 的事件、Resource、依赖和职责边界。

本文档允许使用 LLM、workspace、skill、agent、workflow 等业务概念；通用 ECS 与运行时 API 以
[V3-INFRASTRUCTURE-API.md](V3-INFRASTRUCTURE-API.md) 为准。

现阶段文档统一使用中文，正式发布产品文档时再考虑 i18n。

## 1. 设计目标

业务 Plugin 必须满足：

- 可插拔：移除后不破坏无直接依赖的 Plugin。
- 可测试：同步 Plugin 只依赖 core；异步 Plugin 显式安装 Async runtime。
- 可组合：跨 Plugin 优先通过 Event，其次通过稳定 Resource。
- 可观测：重要状态变化以 Event 暴露。
- 不越界：不读取其他 Plugin 私有模块和内部 channel。
- 不控制全局基础设施：不自行初始化 Tokio、tracing subscriber 或 App 运行循环。
- 开箱即用：产品默认组合由 PluginGroup 提供，常规开发不要求用户手动组装基础设施细节。

## 2. 标准结构

```text
src/
├── lib.rs
├── plugin.rs
├── events.rs
├── resource.rs
└── systems.rs
```

复杂 Plugin 可以拆分 `systems/`，但 `lib.rs` 只导出稳定 API。

允许公开：

- Plugin 类型
- public Event
- public Resource
- Options、领域 Error、必要 Handle/Trait

默认不公开：

- System 函数
- storage 和 channel
- worker、helper、内部中间 Event

## 3. 构造与 build

每个 Plugin 提供零配置构造：

```rust
impl Default for ExamplePlugin {
    fn default() -> Self {
        Self::new()
    }
}
```

构造函数只保存配置，不启动线程、不访问网络、不打开 socket、不读取配置文件。

`Plugin::build(&self, app: &mut App)` 可以：

- 注册 Event、Resource 和同步 System
- 创建 EventReader 并移动进 System
- 使用已声明依赖的 extension trait，例如 `AsyncAppExt`

不能：

- 阻塞 I/O 或长时间计算
- 启动不受 Resource 生命周期管理的线程
- 修改 tracing global subscriber
- 直接访问其他 Plugin 内部状态

## 4. Stage 与顺序

```text
Startup  只执行一次的初始化
First    基础设施保留
Update   业务 System
Last     基础设施保留
```

普通业务 Plugin 只使用 Startup 和 Update。

Input、Prepare、Execute、Finalize、Event 等业务阶段目前不进入 core。需要顺序时使用 named System 的 `before` / `after`。未来业务 runtime 可以定义稳定 SystemSet，但不得修改 core Stage。

## 5. Event 规范

Event 必须：

- `Clone + Send + Sync + 'static`
- 表示已经发生的事实，或命名明确的请求
- 不携带引用、锁、channel 或 API key
- 使用独立 EventReader，不假设事件只能被一个消费者读取

推荐命名：

```text
XxxRequested
XxxStarted
XxxCompleted
XxxFailed
XxxEmitted
```

Request 和结果必须携带稳定关联 ID，不能依赖队列顺序配对。

## 6. Resource 规范

Resource 用于 World 级共享状态：

- 必须 `Send + Sync + 'static`
- 字段默认私有
- 公开方法维护不变量
- 不让调用方长期持有锁
- 对外返回 snapshot、clone 或明确 Handle

Resource 不用于绕过 Event 构造隐式调用链。

## 7. 异步业务规范

所有 ECS System 保持同步。异步 Plugin：

1. 依赖 `async_runtime_plugin`。
2. 在 composition root 中先安装 `AsyncRuntimePlugin`。
3. 通过 `AsyncAppExt` 注册 Request → Future → Output。
4. Future 不借用 World。
5. 领域失败转换为领域 `XxxFailed` Event。

框架级 `AsyncTaskFailed` 用于观察 timeout、cancel、panic、queue full；它不能替代领域失败事件。

## 8. Error 与安全

领域失败使用结构化 kind：

```rust
pub struct XxxFailed {
    pub request_id: String,
    pub kind: XxxFailureKind,
    pub message: String,
}
```

错误和日志禁止包含：

- API key、token、Authorization header
- 完整隐私文件
- 无限制 stdout/stderr
- 不必要的完整 prompt 或模型响应

## 9. Plugin 依赖

```text
Required dependency
  缺失时无法工作，build 可判断则立即报错，否则 Startup 发出失败 Event。

Optional dependency
  缺失时禁用部分能力，不静默替换为不同实现。

Event dependency
  只依赖对方公开 Event 契约。
```

基础设施 Plugin 必须先于依赖它的业务 Plugin 安装。安装顺序不等于同 Stage 的执行顺序。

推荐依赖：

```text
config_plugin    → core_plugin + types
event_bus_plugin → core_plugin + types
llm_plugin       → core_plugin + async_runtime_plugin + providers + types
sandbox_plugin   → core_plugin + async_runtime_plugin + sandbox
skill_plugin     → core_plugin
server_plugin    → core_plugin + http_server_plugin
```

## 10. Compose、WorkspaceBundle 与资源管理边界

### 10.1 CLI 与 daemon 的职责

CLI 是本地项目工具链和 daemon 客户端，可以读取文件、解析 compose、收集资源、执行
预检并发起资源管理命令；daemon 是资源库与运行状态的权威所有者。

```text
CLI owns
  本地 compose / Soul / Skill / Workflow 文件的读取
  相对路径解析和本地 schema 预检
  WorkspaceBundle 构建和上传
  命令交互、结果展示和退出码

daemon owns
  已安装 Agent / Skill / Provider 等资源目录
  Workspace / Request / Task 运行状态
  权威语义校验、安全策略和持久化
  ECS 生命周期与业务状态转换
```

`margatroid agent ls` 等命令通过 API 查询 daemon 资源库，不在 CLI 内维护第二份权威
数据库。CLI 可以保留可删除的下载缓存，但缓存不得成为 workspace 可恢复性的前提。

### 10.2 共享协议对象

CLI 与 daemon 通过 `margatroid_protocol` 共享以下纯数据对象；阶段 3 的 project crate
负责把 compose 编译成这些对象：

```text
WorkspaceSpec       规范化 workspace、Agent、Workflow 和引用关系
WorkspaceBundle     WorkspaceSpec + ResourceManifest + 资源内容
ResourceManifest    每个资源的 kind、逻辑名称、版本和内容哈希
ResourceId          daemon 中已安装资源的稳定 ID
ProjectName         compose 项目名
```

这些类型不得依赖 ECS、Axum、CLI 或 daemon 实现。资源内容必须有大小上限和内容哈希；
daemon 必须独立验证 schema version、hash、引用、路径和安全策略。

Provider secret 不进入 `WorkspaceBundle`、日志或规范化 compose 输出。compose 只允许通过
稳定 Provider ID 引用 daemon 侧凭据。

### 10.3 Compose 编译流程

```text
compose.toml
→ parse + resolve local references
→ local preflight
→ normalized WorkspaceSpec
→ collect local resources into WorkspaceBundle
→ upload
→ daemon authoritative validation
→ persist accepted bundle
→ WorkspacePlugin lifecycle Event
```

CLI 不得把未打包的本地绝对路径交给 daemon 读取。daemon 接受 workspace 前必须确保资源包
可独立恢复；不能依赖 CLI 当前工作目录或本地缓存仍然存在。

### 10.4 资源命令与运行时命令

- `agent/skill/provider ls|inspect|add|remove` 管理 daemon 中的共享资源库。
- `compose up|stop|start|restart|down|ps|logs|config` 管理一个 compose 项目。
- 顶层 `ps/inspect/logs` 管理或观察 daemon 全局运行状态。
- 删除仍被 workspace 引用的资源必须拒绝，除非未来提供语义明确的强制迁移机制。
- `compose down` 不隐式删除共享 Agent、Skill、Provider 或历史数据。

## 11. 当前业务 Plugin 契约

### 11.1 ConfigPlugin

职责：

- 管理配置路径和 ConfigStore
- 加载 daemon 和 provider 运行配置
- 发出加载、重载和失败 Event

公开 Event：

```text
ConfigLoadRequested
ConfigLoaded
ConfigReloaded
ConfigLoadFailed
```

公开 Resource：`ConfigStore`

不负责创建 workspace、provider 或 server。

### 11.2 EventBusPlugin

职责：

- 管理命名广播通道
- 消费 `WorkspaceEventEmitted`
- 提供给 server/CLI 的订阅 handle

公开 Event：

```text
WorkspaceEventEmitted
EventBusPublishFailed
```

公开 Resource：`EventBus`

不定义 runtime 任务状态，不持久化 memory。

### 11.3 LlmPlugin

职责：

- 管理 provider registry
- 消费 `LlmRequest`
- 通过 Async runtime 调用 provider
- 发出 response、stream chunk 和 failure

公开 Event：

```text
LlmRequest
LlmResponse
LlmStreamChunk
LlmFailed
```

公开 Resource：`LlmProviderRegistry`

不决定 agent 路由、不执行 workflow、不写 memory、不发送 SSE。

### 11.4 SandboxPlugin

职责：

- 消费命令请求
- 执行权限和隔离策略
- 返回 stdout/stderr/exit status

公开 Event：

```text
SandboxCommandRequested
SandboxCommandStarted
SandboxCommandCompleted
SandboxCommandFailed
```

公开 Resource：

```text
SandboxPolicy
SandboxExecutor
```

不解释工具语义，不直接读取 LLM 输出。

### 11.5 SkillPlugin

职责：

- 扫描 Markdown skill
- 解析 frontmatter
- 分类 member/workflow skill
- 管理 registry 和 load state

公开 Event：

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

公开 Resource：

```text
SkillRegistry
LoadedSkills
```

不执行 workflow DAG、不调用 LLM、不执行 sandbox command。

### 11.6 ServerPlugin

职责：

- 向 `HttpServerPlugin` 注册 Margatroid HTTP API
- 将 EventBus 订阅转换为 SSE/WebSocket 输出
- 启用日志端点时，将 `LogStream` 转换为经过鉴权的 SSE/WebSocket 输出

当前第一版已实现：

- `/health`
- 带 bearer token 鉴权的可选 `/v1/logs/stream`
- `ShutdownRequested` 消费与 HTTP/App 停止

`POST /v1/workspaces/{workspace}/prompts` 暂不开放。必须等未来 workflow/workspace
Plugin 定义稳定 protocol ingress、安装实际消费 System 并提供明确 capability 后，HTTP
适配层才能注册该路由并返回 `202 Accepted`。缺少消费者时不得接受请求。

携带 request_id 的共享 DTO 和错误码由 `margatroid_protocol` 提供，供 CLI 与 daemon
共同依赖；不放入 `server_plugin` 实现 crate。

公开 Event：

```text
ShutdownRequested
```

公开配置：

```text
ServerPluginOptions
LogEndpointOptions
```

依赖 `HttpServerPlugin` 提供 listener、Router 和优雅停止。日志端点是可选能力，
启用时要求 `LogPlugin` 已启用 Stream Layer。`ServerPlugin` 负责该端点的鉴权、
限流和可见性；`LogPlugin` 不开启 HTTP Server。

不负责 HTTP 服务生命周期、业务调度、LLM 调用、workflow 和 memory。

## 12. 默认产品组合

Margatroid 开发者不需要为常规 daemon 逐个了解和配置基础设施 Plugin。
产品层由 `margatroid_defaults` crate 提供预置 PluginGroup：

```rust
app.add_plugins(MargatroidDaemonPlugins::default());
```

默认组合内部按依赖顺序安装：

```text
LogPlugin
→ AppRuntimePlugin
→ ExternalEventPlugin
→ AsyncRuntimePlugin
→ HttpServerPlugin
→ EventBusPlugin
→ ServerPlugin
→ 其他 Margatroid 业务 Plugin
```

高级用户仍可以拆开 PluginGroup，替换默认配置或移除可选能力。

## 13. 延后设计的业务 Plugin

```text
workspace_plugin
workflow_plugin
member_plugin
memory_plugin
```

这些 API 尚未稳定，但必须遵守：

- System 同步
- 异步通过 Async runtime request/output 契约
- 跨 Plugin 使用 Event 或稳定 Resource
- 不让 LLM 控制程序流程
- 不把领域 Stage 硬编码进 core

## 14. 测试要求

每个业务 Plugin 至少包含：

- Resource 单元测试
- Event chain 测试
- Plugin build smoke test
- 领域失败 Event 测试

异步 Plugin 额外测试：

- 成功、领域失败
- timeout、panic、queue full
- cancel（如果公开支持）
- Drop/shutdown

外部服务测试默认 ignored，通过环境变量读取凭据，禁止在源码中写真实 key。

## 15. README 要求

每个 crate README 至少包含：

- 职责和不负责边界
- public Event / Resource / Options
- 依赖的基础设施 Plugin
- Stage / ordering 注册说明
- 最小组合示例
- 失败和 secret 处理方式

## 16. 最小业务 Plugin 示例

```rust
use core_plugin::{App, Plugin, Stage, World};

#[derive(Default)]
pub struct ExamplePlugin;

#[derive(Clone)]
pub struct ExampleRequested {
    pub id: String,
}

#[derive(Clone)]
pub struct ExampleCompleted {
    pub id: String,
}

impl Plugin for ExamplePlugin {
    fn build(&self, app: &mut App) {
        app.add_event::<ExampleRequested>();
        app.add_event::<ExampleCompleted>();

        let mut reader = app.event_reader::<ExampleRequested>();
        app.add_systems(Stage::Update, [move |world: &mut World| {
            for request in world.read_events(&mut reader) {
                world.send_event(ExampleCompleted { id: request.id });
            }
        }]);
    }
}
```
