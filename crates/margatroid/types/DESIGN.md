# 伪代码格式

```text
板块使用一级标题，一行注明文件；类型和函数使用二级标题。
只写接口、字段和直接行为，不写独立逻辑段或过程记录。
持有关系在文末使用树形图。
```

# 板块格式

```text
# lib        各领域/core/src/lib.rs         Resource、World扩展与公开入口
# components 各领域/core/src/components.rs  所属领域组件
# events     各领域/core/src/events.rs      所属领域事件与oneshot请求
# types pub  各领域/core/src/types.rs       所属领域值及能力接口
# error      各领域/core/src/error.rs       所属领域错误
```

# lib

## 类型

```text
MargatroidConfig -> config_core
WorkspaceDefinitions -> workspace_core
ModelRoutes -> inference_core
WorldClientExt -> client_core
WorldAgentExt -> agent_core
WorldWorkspaceExt -> workspace_core
```

# components

## 类型

```text
Address -> address
Client -> client_core
Agent -> agent_core
Workspace、WorkspaceExecutor -> workspace_core
DriverLuaVm -> lua_runtime_core
MclQueue -> mcl_core
AgentStorage -> storage_core
Tools -> tool_resource_core
Prompts -> prompt_resource_core
Sandboxes、SelectedSandbox -> sandbox_resource_core
```

# events

## 类型

```text
ClientRegistrationResult -> client_core
StartAgent、StopAgent、RestartAgent、AgentException -> agent_core
StartWorkspace、StopWorkspace、RestartWorkspace -> workspace_core
StartDriverLuaVm、StopDriverLuaVm、DriverLuaVmExited -> lua_runtime_core
MclCommand -> mcl_core
StorageRequest -> storage_core
ImportTool -> tool_resource_core
ImportPrompt -> prompt_resource_core
ImportSandbox -> sandbox_resource_core
ToolExecuteRequest -> tool_core
InferenceRequest -> inference_core
```

# types pub

## 类型

```text
AgentState -> agent_core
WorkspaceState、WorkspaceDefinition、线程提交能力 -> workspace_core
LuaProgram、LuaEnvironment、LuaValue -> lua_runtime_core
MclInstruction、MclOperation、MclValue -> mcl_core
StorageOperation、HistoryRecord、StorageValue -> storage_core
AgentDriverManifest、AgentDriverInput -> agent_driver_core
ToolInput、PromptInput、SandboxInput -> 各自资源core
ToolExecuteResponse -> tool_core
DomainRequest、InferenceMessage、InferenceResponse -> inference_core
ClientMessage、ServerMessage、网络DTO -> protocol
```

# error

## 类型

```text
领域错误 -> 对应领域core
协议错误 -> protocol
跨领域错误转换 -> 调用方plugin
```

# 持有关系

```text
应用装配层
└── plugins
    ├── 自己的core
    ├── 所需其他core
    └── mecs及通用库

每个core
└── mecs及通用库

protocol
└── serde等通用库
```
