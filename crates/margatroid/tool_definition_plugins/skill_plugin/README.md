# SkillPlugin

SkillPlugin注册`tool:builtin/skill-loader:latest`，按完整`skill:scope/name:tag`查找`SKILL.md`。
查找顺序为项目、AgentImage、主目录；最高优先级目录存在但无效时直接失败。每次调用重新读取文件，
成功或失败都以关联原`tool_call_id`的工具响应返回。

`SKILL.md`必须以`+++`包围的TOML frontmatter开头，`name`和`description`必填：

```text
+++
name = "code-review"
description = "Review code for correctness and regressions."
+++

# Code Review

Inspect the requested change and report concrete findings.
```

注册时使用`description`构造ToolSpec；调用时只返回frontmatter之后的Markdown正文。
