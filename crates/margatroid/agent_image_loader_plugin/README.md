# AgentImageLoaderPlugin

## 介绍

`AgentImageLoaderPlugin` 是 AgentImage 的资源加载层。它从主目录读取人类可编辑的镜像，
在 ECS 主线程创建或刷新 AgentImage Entity，并挂载四类只读静态数据：身份、Soul、中立模型
配置和默认资源可见性。

Loader 只描述镜像中“有什么”，不决定这些内容“怎么运行”：

- Loader 加载 AgentImage，WorkspacePlugin 创建 AgentInstance。
- Loader 保存模型 ID 文本和参数原值，InferencePlugin 解释参数并验证路由。
- Loader 发现默认可见ResourceId，WorkspacePlugin计算Agent默认可见性，ToolPlugin按动态可见性生成请求工具。
- 默认可见性属于 AgentImage，最终可见性属于 AgentInstance。
- 磁盘目录是权威内容，Entity 保存最近一次成功加载的镜像数据。
- 文件读取走 AsyncRuntime，Entity 只能在 ECS 主线程创建或刷新。

## 镜像目录

```text
~/.margatroid/agent-images/local/coder/latest/
├── agent.toml
├── SOUL.md
└── base.lua
```

`agent.toml`：

```toml
schema_version = 1

[inference]
model = "deepseek-v4-flash"
temperature = 0.7
max_output_tokens = 8192

[[dependencies]]
id = "skill:local/code-review:latest"
source = "/home/user/.margatroid/skills/local/code-review/latest"

[[dependencies]]
id = "tool:local/list-directory:latest"
source = "https://example.com/tools/list-directory.tar"
```

镜像资源ID使用 `image:scope/name:tag`，例如 `image:local/coder:latest`。省略 tag 时规范化为 `latest`。
`model` 是模型路由 ID 文本；Provider、base URL 和 API key 属于 `models.toml`，不能写入镜像。

`dependencies`只声明资源ID和可选来源。来源可以是本机路径或URL，当前Loader只校验并保存，
不负责下载或复制；安装流程会在后续阶段按本机资源查找优先级处理来源。

## 安装

```rust
use std::path::PathBuf;

use agent_image_loader_plugin::AgentImageLoaderPlugin;
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use core_plugin::App;

let data_root = PathBuf::from("/home/user/.margatroid");
let agent_image_loader = AgentImageLoaderPlugin::open(data_root.join("agent-images"))?;

let mut app = App::new();
app.add_plugin(RuntimePlugin::default())
    .add_plugin(AsyncRuntimePlugin)
    .add_plugin(agent_image_loader);
```

官方 daemon 组合负责确定主目录并传入绝对路径。Loader 不根据当前工作目录猜测位置，也不要求
InferencePlugin、WorkspacePlugin或BuiltinToolPlugin已经安装。这里的
`agent_image_loader` 是已经配置好、等待安装到 App 的 `AgentImageLoaderPlugin` 实例。

## 加载镜像

WorkspacePlugin 为 Workspace 文件中的每个 Agent 发送加载请求：

```rust
use agent_image_loader_plugin::LoadAgentImage;
use app_runtime_plugin::WorldEventExt;
use margatroid_types::ResourceId;

world.send_event(LoadAgentImage {
    id: "workspace/demo/coder".into(),
    reference: ResourceId::parse("image:local/coder:latest")?,
});
```

加载不会阻塞当前帧。完成后 Loader 在主线程创建或刷新 AgentImage Entity，再发送结果：

```rust
use agent_image_loader_plugin::{
    AgentImageBaseDriver,
    AgentImageIdentity,
    AgentImageModelConfig,
    AgentImageSoul,
    LoadAgentImageResult,
};
use core_plugin::World;

fn inspect_loaded_images(world: &mut World) {
    for event in world.event_reader::<LoadAgentImageResult>() {
        let image = match &event.result {
            Ok(image) => *image,
            Err(error) => {
                tracing::error!(
                    request_id = %event.id,
                    reference = %event.reference,
                    %error,
                    "agent image load failed"
                );
                continue;
            }
        };

        let identity = world
            .get_component::<AgentImageIdentity>(image)
            .expect("successful load must attach identity");
        let soul = world
            .get_component::<AgentImageSoul>(image)
            .expect("successful load must attach soul");
        let model = world
            .get_component::<AgentImageModelConfig>(image)
            .expect("successful load must attach model config");
        let base_driver = world
            .get_component::<AgentImageBaseDriver>(image)
            .expect("successful load must attach base driver");

        tracing::info!(
            ?image,
            reference = %identity.reference(),
            soul_bytes = soul.as_str().len(),
            model = model.model(),
            base_driver_bytes = base_driver.source().source().len(),
            "agent image loaded"
        );
    }
}
```

真正的消费者是 WorkspacePlugin。它必须等镜像和其他启动依赖全部成功后再创建 AgentInstance，
不能在收到第一个成功结果时留下半成品运行对象。

## Base Driver

`AgentImageBaseDriver`保存镜像根目录`base.lua`的已验证源码。资源依赖由Driver中的
`agent.toml`的`dependencies`声明，Driver中的`IMPORT`只负责引用和组织这些依赖；默认和动态
工具可见性由MCL数组管理。当前实现仍兼容旧镜像目录扫描，迁移完成后移除该兼容路径。

禁用始终优先。合并后的`AgentDefaultVisibility`交给AgentPlugin；AgentPlugin创建实例时复制出
初始`AgentDynamicVisibility`。前者只读，后者代表运行中的实际可见资源。
AgentImage 后续刷新不会改变运行中实例；执行 `workspace reload` 后创建的新实例才使用新默认值。

资源内容不进入可见性数据。WorkspacePlugin同时挂载`AgentToolEnvironment`；每次请求由
WorkspacePlugin遍历动态可见资源并逐个交给BuiltinToolPlugin注册工具；具体隐藏执行器从
`AgentToolEnvironment`取得项目根和镜像根。

```text
项目级 .margatroid/skills/<scope>/<name>
→ AgentImage内置 skills/<scope>/<name>
→ 主目录 ~/.margatroid/skills/<scope>/<name>
```

因此修改已有 Skill 的内容会在下一次加载时生效；添加全新逻辑名称需要 `workspace reload`。

## 模型配置边界

`AgentImageModelConfig` 只读保存模型 ID 文本和 `agent.toml` 中的参数原值。Loader 不验证温度
范围、停止序列业务规则，也不查询模型路由。

WorkspacePlugin 创建 AgentInstance 时把该组件交给 InferencePlugin。InferencePlugin 负责：

```text
AgentImageModelConfig
→ ModelId + InferenceParameters
→ 参数业务验证
→ 项目级 / 全局模型路由检查
→ AgentInferenceSnapshot
```

这样 AgentImageLoaderPlugin 不依赖任何推理实现，未来替换 InferencePlugin 也不需要改变镜像
加载层。

## 失败语义

以下情况会返回 `AgentImageLoadError`：

- 镜像不存在或引用越界。
- AgentImage 顶层或资源名称目录出现 symlink、设备文件或非法结构。
- `agent.toml`、Soul 或内置资源名称数量超过加载限制。
- `agent.toml` 无法解析、版本不支持或模型配置缺少基本字段。
- `SOUL.md` 缺失、不是 UTF-8 或为空。
- 静态文件或资源名称目录在加载期间发生变化。

加载成功不代表推理参数、模型路由或 Skill / Workflow 内容已经通过业务验证。失败不会创建半成品
Entity，也不会覆盖上一次成功加载的镜像内容；WorkspacePlugin 必须让本次启动或重载失败。

完整类型、函数和执行逻辑见 [DESIGN.md](DESIGN.md)。
