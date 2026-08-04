# SkillLoaderPlugin

## 介绍

`SkillLoaderPlugin` 负责加载 Skill 资源，不负责执行 Skill 业务。它读取 AgentInstance 的
`SkillVisibility`，确认名称可见后，按项目级、AgentImage 内置、主目录的顺序查找并解析当前
`SKILL.md`。

关键边界：

- AgentInstance 快照只冻结可见名称，不冻结 Skill 内容。
- 每次加载都重新读取当前 `SKILL.md`，修改后无需重启。
- 最高优先级同名来源损坏时明确失败，不静默降级。
- SkillLoaderPlugin 返回描述和指令，SkillPlugin 决定如何交给模型。
- 脚本、模板和资产不预加载，通过本次命中的 Skill 根按需访问。

## Skill格式

```text
<skill-root>/local/code-review/
├── SKILL.md
├── scripts/
├── templates/
└── assets/
```

```markdown
---
name: code-review
description: Review code changes and report concrete correctness risks.
---

# Code Review

Review the supplied changes and prioritize correctness issues.
```

目录提供逻辑名称 `local/code-review`。Frontmatter 中的 `name` 只写 `code-review`，并且必须
与目录名一致；`description` 用于模型可见的 Skill 描述，正文是调用时使用的完整指令。

## 安装Plugin

```rust
use std::path::PathBuf;

use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use core_plugin::App;
use skill_loader_plugin::SkillLoaderPlugin;

let mut app = App::new();
// SkillLoaderPlugin只需要全局主目录Skill根；项目级和镜像级根属于各AgentInstance。
let skill_loader = SkillLoaderPlugin::open(PathBuf::from(
    "/home/user/.margatroid/skills",
))?;

app.add_plugin(RuntimePlugin::default())
    .add_plugin(AsyncRuntimePlugin)
    .add_plugin(skill_loader);
```

Plugin 不扫描整个 Skill 库，也不依赖 WorkspacePlugin、AgentImageLoaderPlugin、InferencePlugin 或
业务 SkillPlugin。这里的 `skill_loader` 只是准备安装到 App 的 `SkillLoaderPlugin` 实例。

## 创建可见性

WorkspacePlugin 创建 AgentInstance 时合并镜像默认、Workspace 额外和禁用名称：

```rust
use skill_loader_plugin::{SkillSourceRoots, SkillVisibility};

// 可见性只是名称集合：镜像默认与Workspace额外项都启用，禁用项最后移除。
let visibility = SkillVisibility::new()
    .with(image_visibility.skills().cloned())
    .with(workspace_agent.skills.clone())
    .without(workspace_agent.disable_skills.clone());

// 来源位置是独立组件，由WorkspacePlugin根据当前项目和AgentImage确定。
let source_roots = SkillSourceRoots::new(
    project_root.join(".margatroid/skills"),
    agent_image_root.join("skills"),
)?;

world.insert_component(agent, visibility);
world.insert_component(agent, source_roots);
```

这里的 `image_visibility` 是 WorkspacePlugin 从 AgentImage Entity 读取的
`AgentImageDefaultVisibility`。SkillLoaderPlugin 本身不依赖 AgentImageLoaderPlugin；
WorkspacePlugin 同时读取两边的公开 API 并完成最终可见性构造。

`SkillVisibility` 不持有路径，也不读取文件，本质上只是去重后的 `ResourceName` 集合。
`SkillSourceRoots` 才保存磁盘定位：WorkspacePlugin 知道项目目录，并能用主目录和
`AgentImageIdentity` 推导 `agent_image_root`，因此由它创建该组件。主目录查找根对整个 App 共用，
仍只在安装 Plugin 时传一次。

生成规则：

```text
visible = image defaults
visible += workspace skills
visible -= workspace disable_skills
```

可见性与 AgentInstance 生命周期一致，只有 `workspace up/reload` 会重新生成。

## 加载Skill

SkillPlugin 在准备模型请求或执行 Skill 时发送同一种事件：

```rust
use app_runtime_plugin::WorldEventExt;
use margatroid_types::ResourceName;
use skill_loader_plugin::LoadSkill;

world.send_event(LoadSkill {
    id: "request-42/skill/local/code-review".into(),
    agent,
    name: ResourceName::new("local/code-review")?,
});
```

加载顺序固定为：

```text
<project>/.margatroid/skills/local/code-review
→ <agent-image>/skills/local/code-review
→ ~/.margatroid/skills/local/code-review
```

结果携带原 AgentInstance、逻辑名称和本次实际读取内容：

```rust
use core_plugin::World;
use skill_loader_plugin::LoadSkillResult;

fn handle_loaded_skills(world: &mut World) {
    for event in world.event_reader::<LoadSkillResult>() {
        match &event.result {
            Ok(skill) => tracing::info!(
                agent = ?event.agent,
                request_id = %event.id,
                skill = %skill.name(),
                source = ?skill.source(),
                description = skill.description(),
                "skill loaded"
            ),
            Err(error) => tracing::error!(
                agent = ?event.agent,
                request_id = %event.id,
                skill = %event.name,
                %error,
                "skill load failed"
            ),
        }
    }
}
```

SkillPlugin 使用 `description()` 生成当前模型请求里的 Skill 工具描述。模型实际调用 Skill 时应
再次发送 `LoadSkill`，使用调用时最新的 `instructions()`，不能默认复用上一次请求的正文。

## 辅助文件

`scripts/`、`templates/` 和 `assets/` 不会随 `SKILL.md` 一起读入内存。`LoadedSkill::resolve`
把相对路径约束在本次命中的 Skill 根内：

```rust
let script = skill
    .resolve(std::path::PathBuf::from("scripts/review.sh"))
    .await?;
```

解析后的路径可以交给 SandboxPlugin。路径不存在、越界、包含 symlink 或指向特殊文件时返回
`SkillLoadError`。该方法必须在异步任务中调用；实际执行权限、超时和输出限制属于SandboxPlugin。

## 动态行为

```text
修改已有SKILL.md
→ 下一次LoadSkill读取新内容

新增项目级同名Skill
→ 下一次LoadSkill立即覆盖镜像或主目录版本

新增全新逻辑名称
→ 当前SkillVisibility仍不可见
→ workspace reload后进入可见集合
```

读取期间文件变化会返回 `SourceChanged`，由 SkillPlugin 决定是否重试。第一版不使用文件监听
或跨请求缓存，因此缓存不会造成修改延迟生效。

完整类型、函数和执行逻辑见 [DESIGN.md](DESIGN.md)。
