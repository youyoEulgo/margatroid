# skill_plugin

`skill_plugin` 提供 V3 Skill 注册表边界。

## 职责

- 扫描 Skill Markdown 文件。
- 解析 TOML frontmatter。
- 区分成员 Skill 与 Workflow Skill。
- 管理 Skill 注册表和已加载 Skill 状态。

## 公开事件

- `SkillScanRequested`
- `SkillScanned`
- `SkillScanFailed`
- `SkillLoadRequested`
- `SkillLoaded`
- `SkillLoadFailed`
- `SkillUnloadRequested`
- `SkillUnloaded`

## 公开 Resource

- `SkillRegistry`
- `LoadedSkills`

## Stage 注册

- 扫描、加载和卸载请求在 `Stage::Update` 消费。

## 最小示例

```rust
use core_plugin::App;
use skill_plugin::{SkillPlugin, SkillScanRequested};

let mut app = App::new();
app.add_plugins(SkillPlugin::new());
app.world().send_event(SkillScanRequested::new("./skills"));
```

## 边界

该 Plugin 不执行 Workflow DAG、不调用 LLM Provider，也不执行 Sandbox 命令。
