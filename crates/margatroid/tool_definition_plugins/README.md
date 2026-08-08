# Tool Definition Plugins

本目录整理所有`ToolDefinitionProvider` Plugin。普通Tool、Skill、Workflow和未来资源都以
`ResourceRef`进入Agent可见性，再由对应Provider转换成完整Tool；它们共享同一套请求与执行链路。

当前目录：

```text
tool_definition_plugins/
├── skill_plugin/
└── workflow_plugin/
```

目录本身不是一个总控Plugin，也不拥有统一资源注册表。新增工具定义提供方时增加同级目录即可。

当前 SkillPlugin 已实现 `SKILL.md` 的三层查找与 Tool 响应；WorkflowPlugin 只注册可解析的占位 Tool，
不读取或执行 Workflow 正文。
