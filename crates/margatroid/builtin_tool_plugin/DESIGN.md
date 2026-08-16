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
    路由资源注册：读取ToolPlugin定义的AgentToolRegisterRequest
    行为：
        id为空或resource_id为tool:builtin/*时直接发送失败AgentToolRegisterResponse
        type=skill转发SkillRegisterRequest
        type=workflow转发WorkflowRegisterRequest
        type=tool转发LuaToolRegisterRequest
        type=shell转发ShellRegisterRequest
        其他type直接发送AgentToolRegisterResponse::ProviderMissing失败响应

collect_builtin_registration_system(world: &mut World)
    收集注册结果：读取四个执行器的注册响应
    行为：保持id、agent、resource_id和result不变，统一发送AgentToolRegisterResponse
```

## 逻辑

```text
AgentDynamicVisibility
    -> AgentPlugin可见性注册协调器
    -> AgentToolRegisterRequest { resource_id }
    -> BuiltinToolPlugin按resource_id.type路由
       ├── skill:*    -> tool:builtin/skill-loader:latest
       ├── workflow:* -> tool:builtin/workflow-loader:latest
       ├── tool:*     -> tool:builtin/lua-runtime:latest
       └── shell:*    -> tool:builtin/shell:latest
    -> register_agent_tool(resource_id, hidden tool_id, resource ToolTemplate)
    -> AgentToolRegisterResponse
```

## 边界

```text
内建工具是资源解析和执行后端，对LLM、前端和Agent可见性隐藏。
LLM只能看到AgentToolMap中由skill:*、workflow:*、tool:*和shell:*资源衍生出的ToolSpec。
ToolMap.resource_id表示LLM调用语义；ToolMap.tool_id只供ToolPlugin内部路由。
BuiltinToolPlugin不读取或修改Agent可见性；AgentPlugin根据初始化和热插拔操作决定注册哪些资源。
BuiltinToolPlugin的成功响应只表示AgentToolMap注册完成，不表示资源已经可见；AgentPlugin收到并校验回执后才逐项注入AgentDynamicVisibility。
BuiltinToolPlugin只组合内建执行器，不拥有PendingToolCalls、不构造AgentMessage、不执行Inference。
tool:builtin/*不能作为可见资源注册，防止模型绕过资源定义直接调用执行器。
```
