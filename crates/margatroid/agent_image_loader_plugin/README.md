# AgentImagePlugin

## 契约

crate 名称仍为 `agent_image_loader_plugin`。`AgentImagePlugin` 是公开结构体
`AgentImageLoaderPlugin` 的别名，两者安装同一实现。插件从全局镜像根目录异步读取镜像，
在 ECS 主线程将一个 key 挂载到请求指定的目标 `ResourceSet<AgentImage>`。

不创建独立 Image Entity，不挂 `ResourceId`，不使用 ResourceOwners 或 GC。
成功回执为 `Result<(), ImageLoadError>`，表示目标上的镜像 key 已可用；失败保留原 key。

共享条目与模型定义于 `margatroid_types::resource_components`：

```text
AgentImage { base_driver: Arc<str>, origin: Arc<PathBuf>,
             dependencies: Arc<[ResourceID]>, model: AgentImageModelConfig }
AgentImageModelConfig { model: String, temperature: Option<f32>,
                        max_output_tokens: Option<u32>, top_p: Option<f32>, stop: Vec<String> }
```

请求、业务回执、具名异步输出与错误统一定义于 `margatroid_types::agent_image_events`。
Loader 不解释 Workspace 状态机，不启动 Agent，不查询推理路由，也不自动导入依赖。

## 镜像目录与清单

传给 `open` 的路径是镜像根目录本身，不是其上一级 data root：

```text
~/.margatroid/agent-images/local/coder/latest/
├── agent.toml         必需且有界UTF-8
└── base.lua           必需且有界UTF-8
```

只读取这两个自有文件。其他文档、资源子目录或无关 symlink 均不扫描、不拒绝、不加载。
自有文件和根至镜像目录路径链不允许 symlink，期望目录必须为普通目录，文件必须为普通文件。

```toml
schema_version = 1

[inference]
model = "deepseek-v4-flash"
temperature = 0.7
max_output_tokens = 8192
top_p = 0.9
stop = ["DONE"]

[[dependencies]]
id = "tool:local/code-review:latest"

[[dependencies]]
id = "tool:local/list-directory:latest"

[[dependencies]]
id = "prompt:system/soul:latest"
```

资源 ID 使用完整 `type:scope/name:tag` 地址，省略 tag 规范化为 `latest`。
每项 dependency **只允许 id**，`source` 或其他未知依赖字段会被拒绝。
依赖仅保存为地址，不检查本地文件或资源存在性；prompt 不从镜像里的 Markdown 文件读取，
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
use agent_image_loader_plugin::AgentImagePlugin;
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin};
use core_plugin::App;
use resource_id_plugin::ResourcePlugin;

let plugin = AgentImagePlugin::open("/home/user/.margatroid/agent-images")?;
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

`AgentImageLoadLimits` 默认值：manifest 64 KiB、script 4 MiB、model ID 1024 字节、
最多 128 个 stop、单 stop 最多 4096 字节。manifest 和 script 都最多读取上限加一字节后判定超限。

## 请求与目标挂载

```rust
use app_runtime_plugin::WorldEventExt;
use margatroid_types::agent_image_events::LoadAgentImage;
use resource_id_plugin::ResourceID;

let target = world.spawn();
world.send_event(LoadAgentImage {
    id: "workspace/demo/coder".into(),
    target,
    resource_id: ResourceID::parse("image:local/coder:latest")?,
});
```

异步读取完成不意味着业务成功。唯一主线程 System 在挂载时再次检查完整目标 Entity 的 generation，
克隆旧集合，仅插入或替换请求 key，再经受检 `insert_resources` 挂到目标。
目标 index 被新 Entity 复用时，旧结果不能写入新 generation。

```rust
use core_plugin::World;
use margatroid_types::{agent_image_events::LoadAgentImageResult, resource_components::AgentImage};
use resource_id_plugin::ResourceSet;

fn inspect_loaded_images(world: &mut World) {
    for event in world.event_reader::<LoadAgentImageResult>() {
        match &event.result {
            Ok(()) => {
                let image = world.get_component::<ResourceSet<AgentImage>>(event.target)
                    .and_then(|set| set.get(&event.resource_id))
                    .expect("successful load must mount the requested key");
                tracing::info!(model = %image.model.model, bytes = image.base_driver.len(), "image mounted");
            }
            Err(error) => tracing::error!(id = %event.id, %error, "image load failed"),
        }
    }
}
```

每个请求独立读取，不合并不同目标的同 ID。更新一个目标不传播其他目标。
同 target 同 key 并发暂按挂载发生顺序覆盖，不保证最新提交优先。
读取失败、目标失效或挂载失败均不删除旧集合/旧 key；其他 keys 保持不变。

## 监督与错误

外层异步响应使用框架的 `Result<AgentImageReadOutput, AsyncTaskError>`，不是裸输出事件。
共享输出字段为：

```text
request: LoadAgentImage
result: Result<AgentImage, ImageLoadError>
completed: Arc<AtomicBool>
supervision: Arc<dyn Send + Sync>
```

`supervision` 持续拥有插件私有 `CompletionGuard`，从闭包捕获直到输出最后一份 clone 释放。
没有 armed 字段，也不会在读取返回时解除 guard。任务取消、panic 或未消费输出被丢弃时，
最后一份未完成 guard 发送一次关联失败；主线程发送正常业务结果后才设置 completed，
正常释放或重复输出不重复回执。不新增 oneshot 或公共完成 trait，不修改 Workspace 依赖协议。
Runtime 整体关闭后不保证事件继续被消费。

共享 `ImageLoadError` 只有 `message`，无 ErrorKind；插件产生的描述限制为 512 UTF-8 字节。
`AgentImageLoadError` 是该错误的兼容别名。

## 验证

```sh
cargo check -p agent_image_loader_plugin
cargo test -p agent_image_loader_plugin
```

14 项测试覆盖真实目标挂载与其他 keys 保留、多目标独立、成功重载、失败保旧、
目标 generation 复用、忽略非自有文件/目录/symlink、自有 symlink 拒绝、两文件边界与 UTF-8、
依赖 source/非法 ID/模型参数校验、构造无 mkdir 和路径检查，以及 guard 取消、panic、
输出丢弃、重复失败恰好一次回执。

完整六板块契约见 [DESIGN](<DESIGN.md>)。
