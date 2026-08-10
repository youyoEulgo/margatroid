# WorkflowPlugin

## 定位

WorkflowPlugin是`provider="workflow"`的`ToolDefinitionProvider`。每个Workflow资源都被解析成
一个独立`Tool`，与普通Tool和Skill共享同一套构造机制。

不存在全局`run_workflow(name)`工具。Workflow的`ResourceRef`本身就是可见性和调用定义的单元。

## 类型

公开：
```text
WorkflowPlugin：Workflow工具定义与执行Plugin，公开结构体--配置主目录根并注册Provider
    home_root: Arc<PathBuf>--主目录Workflow根，例如~/.margatroid/workflows
    open(home_root: impl Into<PathBuf>) -> Result<Self, WorkflowError>
        打开Plugin：规范化并保存绝对主目录根，不扫描资源库
    impl Plugin for WorkflowPlugin
        build(self, app: &mut App)
            构建插件：公开trait方法，通过ToolPlugin注册provider ID workflow的WorkflowToolProvider

WorkflowErrorKind：Workflow Plugin配置错误分类，公开枚举
    InvalidRoot--主目录Workflow根不是无父级跳转的绝对路径

WorkflowError：Workflow Plugin配置错误，公开结构体
    kind: WorkflowErrorKind--稳定错误分类，私有
    message: String--错误描述，私有
    new(kind: WorkflowErrorKind, message: impl Into<String>) -> Self
        构造错误：私有关联函数，保存配置错误描述
    kind(&self) -> WorkflowErrorKind
        取得分类：公开方法
    message(&self) -> &str
        取得描述：公开方法
    impl Display + Error for WorkflowError
```

私有：
```text
WorkflowToolProvider：Workflow工具定义提供方，私有结构体
    home_root: Arc<PathBuf>
    impl ToolDefinitionProvider for WorkflowToolProvider
        id() -> "workflow"
        provide(environment, name) -> Result<Tool, ToolError>
            确认项目、AgentImage或主目录中存在对应Workflow目录
            为ResourceRef { provider: "workflow", name }构造占位ToolDefinition和handler
```

## 函数

私有：
```text
find_workflow_directory(environment, home_root, name) -> Result<(), ToolError>
    查找Workflow目录：按项目、AgentImage、主目录顺序确认资源存在，不读取目录内容
    最高优先级同名路径存在但不是目录时直接失败，不向后续来源降级

exposed_name(name: &ResourceName) -> Result<String, ToolError>
    构造模型名称：生成workflow_<scope>_<name>并限制为ToolPlugin允许的64字节

normalize_root(path: PathBuf) -> Option<PathBuf>
    规范化主目录根：要求绝对路径、拒绝父级跳转并去除当前目录段
```

## 逻辑

```text
AgentDynamicVisibility包含：
    ResourceRef { provider: "workflow", name: local/review }

准备LLM请求：
    ToolPlugin按provider="workflow"找到WorkflowToolProvider
        -> 确认项目、AgentImage或主目录中存在local/review目录
        -> provider为local/review构造一个占位Tool
        -> Tool.resource保持同一个ResourceRef
        -> AgentPlugin将ToolDefinition加入InferenceCommand.tools

模型调用Workflow：
    ToolPlugin执行占位handler
        -> 不读取Workflow正文，不执行步骤，不修改Agent动态可见性
        -> 返回尚未实现的Message::Tool
        -> AgentPlugin继续发起InferenceCommand
```

当前Workflow不修改AgentDynamicVisibility，也不实现依赖关系。未来实现修改动态可见性时只影响下一次
LLM请求的tools列表；依赖Skill仍使用`ResourceRef { provider: "skill", ... }`，不建立旁路加载规则。
