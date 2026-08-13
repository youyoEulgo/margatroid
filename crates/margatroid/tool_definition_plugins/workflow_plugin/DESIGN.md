# WorkflowPlugin

## 类型

公开：
```text
WorkflowPlugin：Workflow注册与执行Plugin，公开结构体
    home_root: Arc<PathBuf>
    open(home_root: impl Into<PathBuf>) -> Result<Self, WorkflowError>
    impl Plugin for WorkflowPlugin

WorkflowRegisterRequest：Agent Workflow注册请求，公开事件
    id: String
    agent: Entity
    resource_id: ResourceId
    impl Event for WorkflowRegisterRequest

WorkflowRegisterResponse：Agent Workflow注册结果，公开事件
    id: String
    agent: Entity
    resource_id: ResourceId
    result: Result<(), ToolError>
    impl Event for WorkflowRegisterResponse

WorkflowError：Workflow配置错误，公开结构体
WorkflowErrorKind：Workflow配置错误分类，公开枚举
```

## 函数

```text
workflow_register_system(world: &mut World)
    注册Workflow：私有System，读取WorkflowRegisterRequest
    行为：验证完整Workflow ResourceId和资源目录，构造内部ToolTemplate，调用ToolPlugin注册接口写入当前AgentToolMap
    当前阶段：Workflow执行尚未实现；注册是否允许占位模板由Workspace启动策略决定，不伪装成已实现执行器

workflow_tool_call_system(world: &mut World)
    执行Workflow：私有System，读取ToolCallRequest
    行为：只处理tool_id=tool:builtin/workflow-loader:latest；当前返回明确的未实现ToolError并发送ToolCallResponse，不自行发送AgentMessage
```

## 边界

```text
WorkflowPlugin只负责Workflow资源验证、Agent专属模板注册和ToolCallResponse。
ToolPlugin拥有AgentToolMap、PendingToolCalls、响应整理及批次完成判断。
WorkflowPlugin不读取Agent可见性，不保存全局具体Workflow映射，不为静态Agent tag创建目录。
```
