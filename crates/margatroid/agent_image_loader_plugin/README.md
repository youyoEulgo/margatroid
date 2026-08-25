# AgentImageLoaderPlugin

## 介绍

`AgentImageLoaderPlugin` 是 AgentImage 的资源加载层。它从主目录读取人类可编辑的镜像，
在 ECS 主线程创建或刷新 AgentImage Entity。

AgentImage Entity 挂载两类数据：

```text
ResourceId    身份组件，内容为 image:<scope>/<name>:<tag>
AgentImage    聚合组件，保存 Base Driver、依赖清单、模型配置和默认可见性
```

Loader 只描述镜像中“有什么”，不决定这些内容“怎么运行”：

- Loader 加载 AgentImage，WorkspacePlugin 创建 AgentInstance。
- Loader 保存模型 ID 文本和参数原值，InferencePlugin 解释参数并验证路由。
- Loader 发现默认可见 ResourceId，WorkspacePlugin 计算 Agent 默认可见性，ToolPlugin 按动态可见性生成请求工具。
- 默认可见性属于 AgentImage，最终可见性属于 AgentInstance。
- 磁盘目录是权威内容，Entity 保存最近一次成功加载的镜像数据。
- 文件读取走 AsyncRuntime，Entity 只能在 ECS 主线程创建或刷新。

## 镜像目录

```text
~/.margatroid/agent-images/local/coder/latest/
├── agent.toml         必需
├── base.lua           必需
├── SOUL.md            可选，提示词文档
├── TEST.md            可选，提示词文档
├── skills/            可选
├── hooks/             可选
├── tools/             可选
└── shells/            可选
```

`agent.toml`：

```toml
schema_version = 1

[inference]
model = "deepseek-v4-flash"
temperature = 0.7
max_output_tokens = 8192
top_p = 0.9
stop = ["DONE"]

[[dependencies]]
id = "skill:local/code-review:latest"
source = "/home/user/.margatroid/skills/local/code-review/latest"

[[dependencies]]
id = "tool:local/list-directory:latest"
source = "https://example.com/tools/list-directory.tar"

[[dependencies]]
id = "prompt:system/soul:latest"

[[dependencies]]
id = "prompt:system/test:latest"

[[dependencies]]
id = "prompt:user/test:latest"
```

镜像资源 ID 使用 `image:scope/name:tag`，例如 `image:local/coder:latest`。省略 tag 时规范化为 `latest`。
`model` 是模型路由 ID 文本；Provider、base URL 和 API key 属于 `models.toml`，不能写入镜像。

`dependencies` 只声明资源 ID 和可选来源。来源可以是本机路径或 URL，当前 Loader 只校验并保存，
不负责下载或复制；安装流程会在后续阶段按本机资源查找优先级处理来源。

### 提示词文档

提示词依赖以 `prompt:<scope>/<name>:<tag>` 形式声明，Loader 在镜像根目录查找
`<name 大写>.md`：

```text
prompt:system/soul:latest   → SOUL.md
prompt:system/test:latest   → TEST.md
prompt:user/test:latest     → TEST.md
```

`scope` 就是消息类型：只允许 `system` 或 `user`，最终注入上下文时分别作为 `System` 或
`User` 消息。提示词文档本身不关心自己的消息类型，以什么 scope 写进依赖就注入什么类型。

同一份 `.md` 文档可以被多个 prompt 依赖引用，只要 scope 不同；相同 scope 和 name 的 prompt
依赖不允许重复声明，即使 tag 不同。

## 安装

```rust
use std::path::PathBuf;

use agent_image_loader_plugin::AgentImageLoaderPlugin;
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use core_plugin::App;
use resource_id_plugin::ResourceIdPlugin;

let data_root = PathBuf::from("/home/user/.margatroid");
let agent_image_loader = AgentImageLoaderPlugin::open(data_root.join("agent-images"))?;

let mut app = App::new();
app.add_plugin(RuntimePlugin::default())
    .add_plugin(AsyncRuntimePlugin)
    .add_plugin(ResourceIdPlugin)
    .add_plugin(agent_image_loader);
```

官方 daemon 组合负责确定主目录并传入绝对路径。Loader 不根据当前工作目录猜测位置，也不要求
InferencePlugin、WorkspacePlugin 或 ToolPlugin 已经安装。

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
use agent_image_loader_plugin::{AgentImage, LoadAgentImageResult};
use core_plugin::World;
use margatroid_types::ResourceId;

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
            .get_component::<ResourceId>(image)
            .expect("successful load must attach identity");
        let image = world
            .get_component::<AgentImage>(image)
            .expect("successful load must attach AgentImage");

        tracing::info!(
            ?image,
            reference = %identity,
            model = image.model().model(),
            base_driver_bytes = image.base_driver().program().source().len(),
            "agent image loaded"
        );
    }
}
```

真正的消费者是 WorkspacePlugin。它必须等镜像和其他启动依赖全部成功后再创建 AgentInstance，
不能在收到第一个成功结果时留下半成品运行对象。

## Base Driver

`AgentImageBaseDriver` 保存镜像根目录 `base.lua` 的已验证 MCL 程序。资源依赖由 `agent.toml`
的 `dependencies` 声明，`base.lua` 中的 `IMPORT` 只负责引用和组织这些依赖；默认和动态工具
可见性由 MCL 数组管理。

禁用始终优先。合并后的 `AgentDefaultVisibility` 交给 AgentPlugin；AgentPlugin 创建实例时复制出
初始 `AgentDynamicVisibility`。前者只读，后者代表运行中的实际可见资源。
AgentImage 后续刷新不会改变运行中实例；执行 `workspace reload` 后创建的新实例才使用新默认值。

资源内容不进入可见性数据。WorkspacePlugin 同时挂载 `AgentToolEnvironment`；每次请求由
WorkspacePlugin 遍历动态可见资源并交给 ToolPlugin 注册工具；具体执行器从
`AgentToolEnvironment` 取得项目根和镜像根。

```text
项目级 .margatroid/<资源目录>/<scope>/<name>/<tag>
→ AgentImage 内置 <资源目录>/<scope>/<name>/<tag>
→ 主目录 ~/.margatroid/<资源目录>/<scope>/<name>/<tag>
```

资源目录按类型分别为 `skills/`、`hooks/`、`tools/`、`shells/`。修改已有资源内容会在下一次
使用时生效；添加全新逻辑名称需要 `workspace reload`。

## 模型配置边界

`AgentImageModelConfig` 只读保存模型 ID 文本和 `agent.toml` 中的参数原值。Loader 不验证温度
范围、停止序列业务规则，也不查询模型路由。

WorkspacePlugin 创建 AgentInstance 时把该配置交给 InferencePlugin。InferencePlugin 负责：

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
- AgentImage 顶层出现 symlink、设备文件或非法结构。
- `agent.toml`、Base Driver 或目录入口数量超过加载限制。
- `agent.toml` 无法解析、版本不支持或模型配置缺少基本字段。
- prompt 依赖缺少对应的 `<name 大写>.md` 文件。
- prompt 依赖的 scope 不是 `system` 或 `user`。
- 相同 scope 和 name 的 prompt 依赖重复声明。
- 静态文件或资源目录在加载期间发生变化。

加载成功不代表推理参数、模型路由或资源内容已经通过业务验证。失败不会创建半成品 Entity，
也不会覆盖上一次成功加载的镜像内容；WorkspacePlugin 必须让本次启动或重载失败。

完整类型、函数和执行逻辑见 [DESIGN.md](DESIGN.md)。
