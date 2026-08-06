# SkillPlugin

## 定位

SkillPlugin是`provider="skill"`的`ToolDefinitionProvider`。每个Skill资源都被解析成一个独立
`Tool`，与普通Tool走完全相同的构造链路。

不存在全局`use_skill(name)`工具，也不在工具参数中进行第二次资源路由。Skill的`ResourceRef`就是
可见性单元，模型可见工具名直接对应这一个Skill。

## 类型

```text
SkillPlugin：Skill工具定义Plugin，公开结构体--配置主目录Skill根并注册SkillToolProvider
    home_root: Arc<PathBuf>--主目录Skill根，例如~/.margatroid/skills
    open(home_root: impl Into<PathBuf>) -> Result<Self, SkillError>
        打开Plugin：保存绝对主目录根，不扫描资源库
    impl Plugin for SkillPlugin
        行为：确认ToolPlugin已安装，以provider ID skill注册SkillToolProvider

SkillToolProvider：Skill工具定义提供方，私有结构体
    home_root: Arc<PathBuf>
    impl ToolDefinitionProvider for SkillToolProvider
        id() -> "skill"
        provide(environment, name) -> Result<Tool, ToolError>
            为ResourceRef { provider: "skill", name }构造独立ToolDefinition和handler

SkillErrorKind：Skill错误分类，公开枚举
    NotFound
    InvalidPackage
    ReadFailed
    ExecutionFailed
```

## 逻辑

```text
AgentDynamicVisibility包含：
    ResourceRef { provider: "skill", name: local/code-review }

准备LLM请求：
    ToolPlugin按provider="skill"找到SkillToolProvider
        -> provider为local/code-review构造一个Tool
        -> Tool.resource保持同一个ResourceRef
        -> ToolDefinition.name是该Skill稳定、Provider安全的模型调用名
        -> AgentPlugin将definition加入InferenceCommand.tools
```

最高优先级同名目录存在但内容非法时直接失败，不向低优先级降级。SkillPlugin不读取Agent可见性；
它只为ToolPlugin传入的单个ResourceRef提供定义和执行器。ToolCall处理尚未定义。
