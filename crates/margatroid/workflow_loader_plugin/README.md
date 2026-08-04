# WorkflowLoaderPlugin

## 介绍

`WorkflowLoaderPlugin` 是 Workflow 资源加载层。它按 AgentInstance 的 `WorkflowVisibility`
查找当前可见的 Workflow 包，读取清单和入口文件，返回 `LoadedWorkflow`。

它不执行 Workflow，不解释节点类型，也不负责 tool-call loop。Workflow 可以从第一版的 YAML
描述开始，未来替换为新的节点格式或独立语言，而不改变作用域查找和加载 API。

关键边界：

- Workflow 是目录包，不是单文件。
- 可见性属于 AgentInstance，内容每次加载时读取。
- 作用域顺序是项目级、AgentImage 内置、主目录。
- 最高优先级同名包损坏时直接失败，不静默降级。
- Workflow 的 Skill 依赖由 WorkflowPlugin 检查，Loader 只返回依赖名称。
- Loader 不依赖 SkillLoaderPlugin 或 WorkflowPlugin。

## Workflow格式

```text
<workflow-root>/local/review/
├── workflow.toml
├── workflow.yaml
├── scripts/--可选
├── templates/--可选
└── assets/--可选
```

`workflow.toml` 是包清单：

```toml
schema_version = 1
name = "review"
description = "Review a change and summarize concrete risks."
entry = "workflow.yaml"
skills = ["local/code-review"]
```

`name` 只写目录中的名称，不包含作用域；`entry` 必须是包内的相对路径。`skills` 是执行该
Workflow 需要的 Skill 逻辑名称，Loader 只验证名称格式，不自动加载或执行它们。

入口文件的格式不由 Loader 固定。第一版可以使用 YAML，WorkflowPlugin 负责解释节点；未来可以
替换入口语言而不改变资源查找、清单读取和依赖声明。

## 安装Plugin

```rust
use std::path::PathBuf;

use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use core_plugin::App;
use workflow_loader_plugin::WorkflowLoaderPlugin;

let mut app = App::new();
// WorkflowLoaderPlugin只需要全局主目录Workflow根；项目级和镜像级根属于各AgentInstance。
let workflow_loader = WorkflowLoaderPlugin::open(PathBuf::from(
    "/home/user/.margatroid/workflows",
))?;

app.add_plugin(RuntimePlugin::default())
    .add_plugin(AsyncRuntimePlugin)
    .add_plugin(workflow_loader);
```

Plugin 不扫描或预加载整个 Workflow 库，也不要求 WorkflowPlugin、SkillLoaderPlugin 或
WorkspacePlugin 已经安装。这里的 `workflow_loader` 只是准备安装到 App 的
`WorkflowLoaderPlugin` 实例。

## 创建可见性

WorkspacePlugin 创建 AgentInstance 时合并 AgentImage 默认、Workspace 额外和禁用名称：

```rust
use workflow_loader_plugin::{WorkflowSourceRoots, WorkflowVisibility};

// 可见性只是名称集合：镜像默认与Workspace额外项都启用，禁用项最后移除。
let visibility = WorkflowVisibility::new()
    .with(image_visibility.workflows().cloned())
    .with(workspace_agent.workflows.clone())
    .without(workspace_agent.disable_workflows.clone());

// 来源位置是独立组件，由WorkspacePlugin根据当前项目和AgentImage确定。
let source_roots = WorkflowSourceRoots::new(
    project_root.join(".margatroid/workflows"),
    agent_image_root.join("workflows"),
)?;

world.insert_component(agent, visibility);
world.insert_component(agent, source_roots);
```

这里的 `image_visibility` 是 WorkspacePlugin 从 AgentImage Entity 读取的
`AgentImageDefaultVisibility`。WorkflowLoaderPlugin 本身不依赖 AgentImageLoaderPlugin；
WorkspacePlugin 同时读取两边的公开 API 并完成最终可见性构造。

`WorkflowVisibility` 不持有路径，也不读取文件，本质上只是去重后的 `ResourceName` 集合。
`WorkflowSourceRoots` 才保存磁盘定位：WorkspacePlugin 知道项目目录，并能用主目录和
`AgentImageIdentity` 推导 `agent_image_root`，因此由它创建该组件。主目录查找根对整个 App 共用，
仍只在安装 Plugin 时传一次。

生成规则：

```text
visible = image defaults
visible += workspace workflows
visible -= workspace disable_workflows
```

可见性和来源根与 AgentInstance 生命周期一致，只有 `workspace up/reload` 会重新生成。

## 加载Workflow

WorkflowPlugin 在需要执行 Workflow 时发送事件：

```rust
use app_runtime_plugin::WorldEventExt;
use margatroid_types::ResourceName;
use workflow_loader_plugin::LoadWorkflow;

world.send_event(LoadWorkflow {
    id: "request-42/workflow/local/review".into(),
    agent,
    name: ResourceName::new("local/review")?,
});
```

Loader 按以下顺序查找：

```text
<project>/.margatroid/workflows/local/review
→ <agent-image>/workflows/local/review
→ ~/.margatroid/workflows/local/review
```

结果包含清单、入口内容和实际来源：

```rust
use workflow_loader_plugin::LoadWorkflowResult;

fn handle_workflow_loads(world: &mut World) {
    for event in world.event_reader::<LoadWorkflowResult>() {
        let workflow = match &event.result {
            Ok(workflow) => workflow,
            Err(error) => {
                tracing::error!(
                    agent = ?event.agent,
                    request_id = %event.id,
                    workflow = %event.name,
                    %error,
                    "workflow load failed"
                );
                continue;
            }
        };

        tracing::info!(
            agent = ?event.agent,
            request_id = %event.id,
            workflow = %workflow.name(),
            source = ?workflow.source(),
            entry_bytes = workflow.entry_bytes().len(),
            "workflow loaded"
        );
    }
}
```

`WorkflowPlugin` 取得 `workflow.manifest().skill_dependencies()` 后，先检查当前 AgentInstance 的
`SkillVisibility`，再委托 SkillLoaderPlugin 按项目级、AgentImage 内置、主目录的顺序加载。Loader
不会因为依赖缺失而自行执行、跳过或替换 Workflow。

## 入口和辅助文件

入口内容通过 `entry_bytes()` 返回，WorkflowPlugin 自己决定解析方式。脚本、模板和资产不预加载；
需要访问时使用：

```rust
let path = workflow.resolve("scripts/check.sh".into()).await?;
```

`resolve` 只允许包内相对路径，并拒绝父级跳转、symlink、特殊文件和不存在路径。实际执行由
SandboxPlugin 或其他文件消费者负责。

## 动态读取与失败

修改已有可见 Workflow 的清单、入口或辅助文件后，下一次 `LoadWorkflow` 读取最新内容，无需重启
Workspace。添加新的逻辑名称或修改 Workspace 可见性列表需要 `workspace reload`。

以下情况返回 `WorkflowLoadError`：

- Workflow 不可见、不存在或路径越界。
- 清单版本不支持、名称不匹配、描述为空或入口路径非法。
- 入口文件不存在、不是普通文件、超过限制或读取期间发生变化。
- 项目级同名 Workflow 损坏时，不回退到镜像或主目录版本。

完整 API 和执行顺序见 [DESIGN.md](DESIGN.md)。
