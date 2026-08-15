# BuiltinToolPlugin

## 类型

公开：
```text
BuiltinToolPlugin：Margatroid内建工具组合与资源注册路由Plugin，公开结构体
    skill_plugin: SkillPlugin--隐藏skill-loader执行器
    workflow_plugin: WorkflowPlugin--隐藏workflow-loader执行器
    lua_plugin: LuaPlugin--隐藏lua-runtime执行器
    shell_plugin: ShellPlugin--隐藏shell执行器
    open(data_root: impl Into<PathBuf>) -> Result<Self, BuiltinToolError>
        打开组合：要求绝对主目录，从skills、workflows、tools和shells子目录构造四个执行器
    impl Plugin for BuiltinToolPlugin
        构建：依次安装四个执行器，再挂载统一注册路由和响应收集System

BuiltinResourceRegisterRequest：可见资源注册请求，公开事件
    id: String
    agent: Entity
    resource_id: ResourceId--LLM可见资源身份，不允许tool:builtin执行器身份
    impl Event for BuiltinResourceRegisterRequest

BuiltinResourceRegisterResponse：可见资源注册响应，公开事件
    id: String
    agent: Entity
    resource_id: ResourceId
    result: Result<(), ToolError>
    impl Event for BuiltinResourceRegisterResponse

BuiltinToolError：组合构造错误，公开结构体
BuiltinToolErrorKind：InvalidRoot、ChildPluginInvalid
```

私有：
```text
BuiltinToolPluginInstalled：重复安装标记，私有Resource
```

## 函数

```text
builtin_resource_register_system(world: &mut World)
    路由资源注册：读取BuiltinResourceRegisterRequest
    行为：
        id为空或resource_id为tool:builtin/*时直接发送失败BuiltinResourceRegisterResponse
        type=skill转发SkillRegisterRequest
        type=workflow转发WorkflowRegisterRequest
        type=tool转发LuaToolRegisterRequest
        type=shell转发ShellRegisterRequest
        其他type直接发送ProviderMissing失败响应

collect_builtin_registration_system(world: &mut World)
    收集注册结果：读取四个执行器的注册响应
    行为：保持id、agent、resource_id和result不变，统一发送BuiltinResourceRegisterResponse
```

## 逻辑

```text
AgentDynamicVisibility
    -> WorkspacePlugin
    -> BuiltinResourceRegisterRequest { resource_id }
    -> BuiltinToolPlugin按resource_id.type路由
       ├── skill:*    -> tool:builtin/skill-loader:latest
       ├── workflow:* -> tool:builtin/workflow-loader:latest
       ├── tool:*     -> tool:builtin/lua-runtime:latest
       └── shell:*    -> tool:builtin/shell:latest
    -> register_agent_tool(resource_id, hidden tool_id, resource ToolTemplate)
    -> BuiltinResourceRegisterResponse
```

## 边界

```text
内建工具是资源解析和执行后端，对LLM、前端和Agent可见性隐藏。
LLM只能看到AgentToolMap中由skill:*、workflow:*、tool:*和shell:*资源衍生出的ToolSpec。
ToolMap.resource_id表示LLM调用语义；ToolMap.tool_id只供ToolPlugin内部路由。
BuiltinToolPlugin不读取Agent可见性；WorkspacePlugin决定注册哪些资源。
BuiltinToolPlugin只组合内建执行器，不拥有PendingToolCalls、不构造AgentMessage、不执行Inference。
tool:builtin/*不能作为可见资源注册，防止模型绕过资源定义直接调用执行器。
```
