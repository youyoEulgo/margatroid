# AgentDriverPlugin

## 仓库资源

仓库 `resources/` 保存可版本管理的资源包：`agent-drivers/local/{coder,reviewer}/latest/driver.toml` 与 `base.lua` 为两个 Driver，`tools/`、`prompts/`、`sandboxes/` 保存现有全局资源。Prompt 的 `content.md` 是运行时资源正文，不是额外说明文档。

这些包不是新的隐式加载来源：daemon 仍按全局根加载，使用前应将资源部署到对应的 `~/.margatroid/agent-drivers`、`tools`、`prompts`、`sandboxes` 目录。模型路由与凭据不包含在资源包内，仍由使用者独立配置。测试显式指定仓库资源路径，不依赖本机部署包。

本次仅将 AgentImage 术语改为 AgentDriver，地址为 `driver:`、工作区字段为 `driver`、清单为 `driver.toml`；Workspace 持有定义集合、Agent 选择定义和各自运行实例的逻辑不变。

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

请求、业务回执、具名异步输出与错误统一定义于 `margatroid_types::agent_driver_events`。
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
依赖仅保存为地址，不检查本地文件或资源存在性；prompt 不从驱动里的 Markdown 文件读取，
不限制 prompt scope，不按 scope/name 去重。不从模型配置反向推断依赖。

清单需 `schema_version = 1`。模型名 trim 后不能为空，长度受限且无控制字符。
可选 temperature 必须有限并在 `[0, 2]`，top_p 必须有限并在 `[0, 1]`，
max_output_tokens 不能为 0。stop 数量及单项字节长度受限。
模型路由是否存在、backend 是否可用不属于 Loader 校验。

`base.lua` 保存为普通 UTF-8 源码和 origin 路径。不计算 source hash、不编译或执行 Lua，
不解析 IMPORT、INJECT 或默认可见性。读取不承诺两个文件的原子版本快照，
也不提供恶意并发路径替换场景下无 TOCTOU 的文件打开保证。

## 安装

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

`open` 要求绝对路径且无父级跳转，消除 `.`，不创建目录也不预扫描。
加载请求时才检查根是否存在。`with_schedule` 和 `with_limits` 可配置计划及读取限制。
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

16 项测试覆盖真实目标挂载与其他 keys 保留、多目标独立、成功重载、失败保旧、
目标 generation 复用、忽略非自有文件/目录/symlink、自有 symlink 拒绝、两文件边界与 UTF-8、
依赖 source/非法 ID/模型参数校验、构造无 mkdir 和路径检查，以及 guard 取消、panic、
输出丢弃、重复失败恰好一次回执。

完整六板块契约见 [DESIGN](<DESIGN.md>)。

## 可操作的加载诊断

加载失败必须保留完整 ResourceID、实际尝试的绝对文件或目录路径，以及底层 I/O、UTF-8、清单解析或校验原因；不得用笼统的“读取失败”覆盖原因，也不得在添加上下文时截断路径或原因。逐祖先检查失败同时保留最终尝试路径和失败的祖先路径。

依赖加载器在原有错误外添加资源上下文，保留上层调用方已经提供的依赖链（包括请求 id 中的关联链）；本插件不凭空推断驱动来源，也不递归加载依赖。调用方应将 `driver -> dependency` 链追加在本错误之外；Driver 清单声明的依赖仍只解析地址，不检查存在性。缺失依赖由对应 tool、prompt 或 sandbox 加载器报告其自身完整地址及实际绝对路径。

错误在异步读取、主线程挂载及业务回执之间传播时保留原分类和原因；诊断文本不再受插件的旧 512 字节/字符截断限制，公共错误类型不变。回归测试覆盖缺失驱动目录、缺失 driver.toml/base.lua，以及缺失各类依赖文件。
