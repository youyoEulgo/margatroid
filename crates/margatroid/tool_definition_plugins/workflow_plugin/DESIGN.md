# WorkflowPlugin

## 定位

WorkflowPlugin是`provider="workflow"`的`ToolDefinitionProvider`。每个Workflow资源都被解析成
一个独立`Tool`，与普通Tool和Skill共享同一套构造机制。

不存在全局`run_workflow(name)`工具。Workflow的`ResourceRef`本身就是可见性和调用定义的单元。

## 类型

```text
WorkflowPlugin：Workflow工具定义与执行Plugin，公开结构体--配置主目录根并注册Provider
    home_root: Arc<PathBuf>--主目录Workflow根，例如~/.margatroid/workflows
    open(home_root: impl Into<PathBuf>) -> Result<Self, WorkflowError>
        打开Plugin：保存绝对主目录根，不扫描资源库
    impl Plugin for WorkflowPlugin
        行为：确认ToolPlugin已安装，以provider ID workflow注册WorkflowToolProvider

WorkflowToolProvider：Workflow工具定义提供方，私有结构体
    home_root: Arc<PathBuf>
    impl ToolDefinitionProvider for WorkflowToolProvider
        id() -> "workflow"
        provide(environment, name) -> Result<Tool, ToolError>
            为ResourceRef { provider: "workflow", name }构造独立ToolDefinition和handler

WorkflowErrorKind：Workflow错误分类，公开枚举
    NotFound
    InvalidPackage
    ReadFailed
    ExecutionFailed
```

## 逻辑

```text
AgentDynamicVisibility包含：
    ResourceRef { provider: "workflow", name: local/review }

准备LLM请求：
    ToolPlugin按provider="workflow"找到WorkflowToolProvider
        -> provider为local/review构造一个Tool
        -> Tool.resource保持同一个ResourceRef
        -> AgentPlugin将ToolDefinition加入InferenceCommand.tools
```

Workflow修改AgentDynamicVisibility后，只影响下一次LLM请求的tools列表。Workflow依赖Skill时使用
`ResourceRef { provider: "skill", ... }`加入统一动态可见性，不建立旁路加载规则。ToolCall处理尚未定义。
