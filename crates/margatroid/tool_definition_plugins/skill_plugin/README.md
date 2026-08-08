# SkillPlugin

SkillPlugin注册`provider="skill"`的`ToolDefinitionProvider`。每个可见Skill都是一个独立Tool，
不存在额外的`use_skill(name)`路由。是否进入模型请求完全由Agent动态可见性决定。

当前实现只处理 `SKILL.md`。每次 ToolPlugin 构造 Skill 工具时，按以下顺序查找：

```text
<project>/.margatroid/skills/<scope>/<name>/SKILL.md
<agent-image>/skills/<scope>/<name>/SKILL.md
<home-root>/<scope>/<name>/SKILL.md
```

最高优先级文件存在时直接读取，不合并不同来源。文件正文同时作为 `ToolDefinition.description` 和工具
调用结果；参数暂时接受任意 JSON object，不执行脚本、不读取其他文件，也不解析 Skill frontmatter。

```rust
use skill_plugin::SkillPlugin;

let skills = SkillPlugin::open("/home/user/.margatroid/skills")?;
app.add_plugin(tool_plugin::ToolPlugin::default())
    .add_plugin(skills);
# Ok::<(), Box<dyn std::error::Error>>(())
```
