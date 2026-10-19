# AgentDriverPlugin

## 当前状态与可用驱动清单

当前 catalog、注册启动、成员增删、工作区创建/改名/删除、成员改名、Driver 发现及后端目录浏览均已实现并完成恢复验证。验证仅覆盖已执行的 scoped、真实存储集成和隔离数据浏览器场景，不构成所有故障或环境的普遍保证。

Sidebar 新建工作区输入名称及已存在的绝对规范项目目录；新建会话输入名称并显式选择一个已有 AgentDriver。清单在启动时扫描有效已安装包，包含尚未挂载到任何 Workspace 的包，使用 `driver:scope/name:tag`（不是 `agent-driver:`）。没有默认驱动或自动加载语义。有效包要求普通文件与路径检查、有界 UTF-8 和合法清单 schema/model/dependencies；不执行 Lua，也不承诺模型路由或依赖运行可用。加载时仍重新读取校验并显式挂载。

可用驱动清单已实现：发现所有已安装包（包括未挂载包），校验普通路径/文件、有界 UTF-8、清单 schema_version、模型参数和 dependencies 地址；无效包不进入 drivers，逐包诊断保留。这里的“有效”不包含 Lua 语法编译或执行、依赖存在性、模型路由或运行可用性。加载时仍重新校验。

## 仓库资源

仓库 `resources/` 保存可版本管理的资源包：`agent-drivers/local/{coder,reviewer}/latest/driver.toml` 与 `base.lua` 为两个 Driver，`tools/`、`prompts/`、`sandboxes/` 保存现有全局资源。Prompt 的 `content.md` 是运行时资源正文，不是额外说明文档。

这些包不是新的隐式加载来源：daemon 仍按全局根加载，使用前应将资源部署到对应的 `~/.margatroid/agent-drivers`、`tools`、`prompts`、`sandboxes` 目录。模型路由与凭据不包含在资源包内，仍由使用者独立配置。测试显式指定仓库资源路径，不依赖本机部署包。

统一术语为 AgentDriver，地址为 `driver:`、工作区字段为 `driver`、清单为 `driver.toml`。Driver 包只含行为源码与清单契约，不提供包内嵌入资源、Skill、Hook 或工作流加载机制；tool、prompt、sandbox始终由各自全局资源加载器读取。额外目录项被忽略不表示支持包内资源。

## AgentDriver 是什么“image”

这里的 image 是一个**可选的驱动定义快照**，不是容器、磁盘镜像或进程：它只是驱动根目录下一个由 `driver.toml` 与 `base.lua` 组成的目录，地址形如 `driver:<scope>/<name>:<tag>`。Agent 不硬编码行为，而是在创建成员时**从这些 image 中选择一个**；同一 image 可被多个 Agent、多个 Workspace 同时选用，选择只记录其 `driver:` 地址。不同 tag 是不同可选项，可并存。

因此“image”在系统中只有两层含义：磁盘上的一次静态目录快照，以及运行时被加载进目标 `ResourceSet<AgentDriver>` 的一个 key。选择 image 不等于启动 Agent，也不等于推理路由可用。catalog 只是**可选项清单**，不是隐式加载来源：即使列在 catalog 中，仍须显式请求加载才会读取文件并挂载。


## 契约

crate 名称仍为 `agent_driver_loader_plugin`。`AgentDriverPlugin` 是公开结构体
`AgentDriverLoaderPlugin` 的别名，两者安装同一实现。插件从全局驱动根目录异步读取驱动，
在 ECS 主线程将一个 key 挂载到请求指定的目标 `ResourceSet<AgentDriver>`。

不创建独立 Driver Entity，不挂 `ResourceId`，不使用 ResourceOwners 或 GC。
成功回执为 `Result<(), DriverLoadError>`，表示目标上的驱动 key 已可用；失败保留原 key。

共享条目与模型定义于 `margatroid_types::resource_components`：

```text
AgentDriver { base_driver: Arc<str>, origin: Arc<PathBuf>,
             dependencies: Arc<[ResourceID]>, model: AgentDriverModelConfig }
AgentDriverModelConfig { model: String, temperature: Option<f32>,
                        max_output_tokens: Option<u32>, top_p: Option<f32>, stop: Vec<String> }
```

加载请求、业务回执、具名异步输出与错误定义于 `margatroid_types::agent_driver_events`；目录发现请求/结果与快照定义于 `margatroid_types::discovery`。
Loader 不解释 Workspace 状态机，不启动 Agent，不查询推理路由，也不自动导入依赖。

## 驱动目录与清单

传给 `open` 的路径是驱动根目录本身，不是其上一级 data root：

```text
~/.margatroid/agent-drivers/local/coder/latest/
├── driver.toml         必需且有界UTF-8
└── base.lua           必需且有界UTF-8
```

只读取这两个自有文件。其他文档、资源子目录或无关 symlink 均不扫描、不拒绝、不加载。
自有文件和根至驱动目录路径链不允许 symlink，期望目录必须为普通目录，文件必须为普通文件。

```toml
schema_version = 1

dependencies = [
    "tool:local/code-review:latest",
    "tool:local/list-directory:latest",
    "prompt:system/soul:latest",
]

[inference]
model = "deepseek-v4-flash"
temperature = 0.7
max_output_tokens = 8192
top_p = 0.9
stop = ["DONE"]
```

资源 ID 使用完整 `type:scope/name:tag` 地址，省略 tag 规范化为 `latest`。
`dependencies` 是顶层字符串数组，放在 `[inference]` 之前（TOML 表头后的字段属于该表）。省略或 `[]` 表示无依赖；每项必须是资源 ID 字符串，旧式依赖表和内联对象不再接受。保留原顺序及重复项，不改变导入、可见工具或沙箱选择语义。
实际反序列化类型是 `Vec<String>`；当前清单结构没有deny_unknown_fields，未知字段可能被忽略，不能将其描述为显式拒绝所有旧字段。忽略字段不赋予嵌入资源或source加载能力。
依赖仅保存为地址，不检查本地文件或资源存在性；prompt 不从驱动里的 Markdown 文件读取，
不限制 prompt scope，不按 scope/name 去重。不从模型配置反向推断依赖。

清单需 `schema_version = 1`。模型名 trim 后不能为空，长度受限且无控制字符。
可选 temperature 必须有限并在 `[0, 2]`，top_p 必须有限并在 `[0, 1]`，
max_output_tokens 不能为 0。stop 数量及单项字节长度受限。
模型路由是否存在、backend 是否可用不属于 Loader 校验。

`base.lua` 保存为普通 UTF-8 源码和 origin 路径。不计算 source hash、不编译或执行 Lua，
不解析 IMPORT、INJECT 或默认可见性。读取不承诺两个文件的原子版本快照，
也不提供恶意并发路径替换场景下无 TOCTOU 的文件打开保证。

## 验证式目录扫描

```rust
use agent_driver_loader_plugin::AgentDriverLoaderPlugin;
use margatroid_types::workspace_catalog::DriverCatalog;

let plugin = AgentDriverLoaderPlugin::open("/home/user/.margatroid/agent-drivers")?;
// 阻塞校验文件与清单，不编译/执行 Lua，不触碰 World/Entity/事件。
let available = plugin.scan_catalog()?;           // Vec<ResourceID>，已排序
let catalog = DriverCatalog(available.clone());
// 可用于 ECS 外部只读校验；插件 build 自行启动异步扫描并发布。
```

`scan_catalog` 是给 ECS 之外调用方的阻塞验证入口，在新线程的 runtime 中调用 `scan_validated`，只返回有效 ID。需要逐包诊断时使用异步 `scan_validated(root, limits)` 返回 AgentDriverCatalogSnapshot。扫描遍历普通 scope/name/tag 目录，跳过子 symlink 和非法地址，根路径链必须为普通目录；缺失/不可读根是错误而非成功空列表。每层最多检查 4096 项，总候选包最多 4096；候选逐包复用读取校验，结果排序去重，不挂载或执行。

## 安装

`build` 安装空快照（若尚无快照）、加载配置和 catalog 系统，并异步发起启动扫描，不在 build 主线程扫描文件。`driver_catalog_system` 处理 query、refresh 和异步完成；成功替换 DriverCatalog 与带诊断快照。首次扫描结束前清单可为空，顶层失败保留最后确认值。插件不创建 Driver Entity；加载仍按明确请求执行。

```rust
use agent_driver_loader_plugin::AgentDriverPlugin;
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin};
use core_plugin::App;
use resource_id_plugin::ResourcePlugin;

let plugin = AgentDriverPlugin::open("/home/user/.margatroid/agent-drivers")?;
let mut app = App::new();
app.add_plugin(RuntimePlugin::default())
    .add_plugin(AsyncRuntimePlugin)
    .add_plugin(ClosurePlugin)
    .add_closure_system(RuntimePlugin::PRE_UPDATE)
    .add_plugin(ResourcePlugin)
    .add_plugin(plugin);
```

`open` 要求绝对路径且无父级跳转，消除 `.`，不创建目录也不做任何 I/O。
根存在性在 `scan_catalog` 或加载请求时检查；插件安装时异步扫描，之后可 query/refresh。`with_schedule` 和 `with_limits` 可配置计划及读取限制。
插件要求应用已经为目标计划注册 ClosureSystem，不重复安装；不依赖 Workspace 或 Inference 安装标记。

`AgentDriverLoadLimits` 默认值：manifest 64 KiB、script 4 MiB、model ID 1024 字节、
最多 128 个 stop、单 stop 最多 4096 字节。manifest 和 script 都最多读取上限加一字节后判定超限。

## 请求与目标挂载

```rust
use app_runtime_plugin::WorldEventExt;
use margatroid_types::agent_driver_events::LoadAgentDriver;
use resource_id_plugin::ResourceID;

let target = world.spawn();
world.send_event(LoadAgentDriver {
    id: "workspace/demo/coder".into(),
    target,
    resource_id: ResourceID::parse("driver:local/coder:latest")?,
});
```

异步读取完成不意味着业务成功。唯一主线程 System 在挂载时再次检查完整目标 Entity 的 generation，
克隆旧集合，仅插入或替换请求 key，再经受检 `insert_resources` 挂到目标。
目标 index 被新 Entity 复用时，旧结果不能写入新 generation。

```rust
use core_plugin::World;
use margatroid_types::{agent_driver_events::LoadAgentDriverResult, resource_components::AgentDriver};
use resource_id_plugin::ResourceSet;

fn inspect_loaded_drivers(world: &mut World) {
    for event in world.event_reader::<LoadAgentDriverResult>() {
        match &event.result {
            Ok(()) => {
                let driver = world.get_component::<ResourceSet<AgentDriver>>(event.target)
                    .and_then(|set| set.get(&event.resource_id))
                    .expect("successful load must mount the requested key");
                tracing::info!(model = %driver.model.model, bytes = driver.base_driver.len(), "driver mounted");
            }
            Err(error) => tracing::error!(id = %event.id, %error, "driver load failed"),
        }
    }
}
```

每个请求独立读取，不合并不同目标的同 ID。更新一个目标不传播其他目标。
同 target 同 key 并发暂按挂载发生顺序覆盖，不保证最新提交优先。
读取失败、目标失效或挂载失败均不删除旧集合/旧 key；其他 keys 保持不变。

## 监督与错误

外层异步响应使用框架的 `Result<AgentDriverReadOutput, AsyncTaskError>`，不是裸输出事件。
共享输出字段为：

```text
request: LoadAgentDriver
result: Result<AgentDriver, DriverLoadError>
completed: Arc<AtomicBool>
supervision: Arc<dyn Send + Sync>
```

`supervision` 持续拥有插件私有 `CompletionGuard`，从闭包捕获直到输出最后一份 clone 释放。
没有 armed 字段，也不会在读取返回时解除 guard。任务取消、panic 或未消费输出被丢弃时，
最后一份未完成 guard 发送一次关联失败；主线程发送正常业务结果后才设置 completed，
正常释放或重复输出不重复回执。不新增 oneshot 或公共完成 trait，不修改 Workspace 依赖协议。
Runtime 整体关闭后不保证事件继续被消费。

共享 `DriverLoadError` 只有 `message`，无 ErrorKind；插件产生的描述完整保留资源、路径和底层原因。
`AgentDriverLoadError` 是该错误的兼容别名。

## 验证

```sh
cargo check -p agent_driver_loader_plugin
cargo test -p agent_driver_loader_plugin
```

Driver scoped 验证 27 passed，包含验证式目录扫描、替换发布、异步启动扫描、query/refresh，以及原有目标挂载、失败保旧、generation 和完成监督。该结果不证明 Lua 能执行或模型能推理。

完整七板块契约见 [DESIGN](<DESIGN.md>)。

## 可操作的加载诊断

加载失败必须保留完整 ResourceID、实际尝试的绝对文件或目录路径，以及底层 I/O、UTF-8、清单解析或校验原因；不得用笼统的“读取失败”覆盖原因，也不得在添加上下文时截断路径或原因。逐祖先检查失败同时保留最终尝试路径和失败的祖先路径。

依赖加载器在原有错误外添加资源上下文，保留上层调用方已经提供的依赖链（包括请求 id 中的关联链）；本插件不凭空推断驱动来源，也不递归加载依赖。调用方应将 `driver -> dependency` 链追加在本错误之外；Driver 清单声明的依赖仍只解析地址，不检查存在性。缺失依赖由对应 tool、prompt 或 sandbox 加载器报告其自身完整地址及实际绝对路径。

DriverLoadError 保存完整 message，异步读取和挂载按原请求关联传播。Tool、Prompt、Sandbox 的共享 ResourceLoadError 构造器仍可能截断到 512 个 Unicode 字符，因此跨依赖链完整诊断仍有真实缺口；本次不扩张为所有诊断路径已验证。


## 可用Driver目录查询与刷新（已实现）

新建Agent必须输入名称，并显式选择已经有效安装的Driver作为image；不能隐式取第一个、使用默认Driver，或把只存在于旧挂载集合的地址当作有效安装证据。启动发现覆盖从未挂载的包，并发布DriverCatalog与 `state.sync.drivers`；对外drivers只包含规范 `driver:scope/name:tag` 字符串，不发送模型配置、凭据、脚本文本或依赖内容。

当前协议为 `driver.catalog.query` / `driver.catalog.refresh`，回复 `driver.catalog {id,drivers,errors:[{path,message}]}`。共享 discovery 模块定义 QueryAgentDriverCatalog、RefreshAgentDriverCatalog、AgentDriverCatalogResult 和快照。query 读快照，refresh 异步复验；成功按扫描序号发布完整替换快照，移除失效/消失安装项，旧扫描不会覆盖较新成功发布。顶层扫描失败保留最后确认快照并返回错误；逐包失败则发布其他有效包及诊断。

有效包验证包括根与必要路径/普通文件限制、读取上限、UTF-8、driver.toml schema_version、inference/model 参数及顶层 dependencies 字符串地址；不编译或执行 Lua，不验证依赖存在性或模型可用性，不自动挂载或启动 Agent。

`workspace.paths.query` 属于daemon的独立类型化目录浏览服务，不由Driver或tool/prompt/sandbox资源加载器承担。集中式Workspace定义保存在 `~/.margatroid/workspaces/<name>/workspace.yaml`，成员数据库为各自 `agents/<agent-name>/state.sqlite`；项目目录只作为工作目录，目标已移除manager。当前 DTO 已移除 manager 预检，空成员工作区合法。
