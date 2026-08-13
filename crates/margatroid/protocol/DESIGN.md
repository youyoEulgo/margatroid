# Protocol

## 类型
公开：
```text
ToolCall：领域工具调用，公开结构体
    id: String
    resource: ResourceId--完整领域资源ID，模型工具别名不得继续使用
    arguments: String

ToolDefinition：模型侧工具定义，公开结构体
    name: String
    description: String
    input_schema: serde_json::Value
```

## 逻辑
```text
领域ToolCall的resource保存完整skill/workflow/tool ResourceId；模型协议名称只在InferencePlugin边界使用。
ResourceId统一格式为type:scope/name:tag，省略tag时解析为latest。
静态Workspace Agent固定使用agent:<workspace>/<name>:latest；clone tag不创建目录，动态Subagent留待后续设计。
```
