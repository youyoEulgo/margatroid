# WorkflowPlugin

## 类型
公开：
```text
WorkflowPlugin：Workflow工具定义Plugin，公开结构体--注册workflow Loader并处理路由事件
    home_root: Arc<PathBuf>
    open(home_root: impl Into<PathBuf>) -> Result<Self, WorkflowError>
    impl Plugin for WorkflowPlugin
WorkflowError：Workflow配置错误，公开结构体
WorkflowErrorKind：Workflow配置错误分类，公开枚举
```

## 函数
```text
workflow_tool_definition_system(world: &mut World)
    定义检查：读取ToolDefinitionRoute；只处理workflow-loader；验证完整workflow ResourceId并返回ToolDefinitionResult
    当前行为：Workflow尚未实现，合法资源返回占位定义，非法资源返回错误

workflow_tool_call_system(world: &mut World)
    Workflow调用：读取ToolCallEvent；只处理workflow-loader；返回占位AgentMessage::Tool，不执行Workflow
```

## 边界
```text
WorkflowPlugin只负责Workflow资源自身的检查和调用响应；ToolPlugin只负责Loader模板注册与事件路由。
不为每个Workflow注册独立工具条目，不读取Agent可见性，不为静态Agent tag创建目录。
```
