# BuiltinToolPlugin

## 类型

公开：
```text
BuiltinToolPlugin：Margatroid内建工具组合与资源注册路由Plugin，公开结构体
    skill_plugin: SkillPlugin--隐藏skill-loader执行器
    lua_plugin: LuaPlugin--隐藏lua-runtime执行器
    shell_plugin: ShellPlugin--隐藏shell执行器
    open(data_root: impl Into<PathBuf>) -> Result<Self, BuiltinToolError>
        打开组合：要求绝对主目录，从skills、tools和shells子目录构造三个执行器
    impl Plugin for BuiltinToolPlugin
        构建：依次安装三个执行器，再挂载统一注册路由和响应收集System

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
    路由资源注册：读取ToolPlugin定义的AgentResourceRegisterRequest
    行为：
        id为空或resource_id为tool:builtin/*时直接发送失败AgentResourceRegisterResponse
        type=skill转发保留alias的SkillRegisterRequest
        type=tool转发保留alias的LuaToolRegisterRequest
        type=shell转发保留alias的ShellRegisterRequest
        type=prompt不作为工具路由，由MclPlugin的Prompt资源解析器处理
        其他type直接发送AgentResourceRegisterResponse::ProviderMissing失败响应

collect_builtin_registration_system(world: &mut World)
    收集注册结果：读取三个执行器的注册响应
    行为：保持id、agent、resource_id、alias和Result<ResourceMapEntry, ToolError>不变，统一发送AgentResourceRegisterResponse
```

## 逻辑

```text
MCL IMPORT
    -> AgentResourceRegisterRequest { resource_id, alias }
    -> BuiltinToolPlugin按resource_id.type路由
       ├── skill:*    -> tool:builtin/skill-loader:latest
       ├── tool:*     -> tool:builtin/lua-runtime:latest
       └── shell:*    -> tool:builtin/shell:latest
    -> AgentResourceRegisterResponse { candidate ResourceMapEntry }
    -> MclPlugin调用register_agent_resource并提交IMPORT事务
```

## 边界

```text
内建工具是资源解析和执行后端，对LLM、前端和Agent可见性隐藏。
LLM只能看到AgentResourceMap中由skill:*、tool:*和shell:*资源衍生出的ToolSpec。
ResourceMapEntry.resource_id表示LLM调用语义；ResourceMapEntry.tool_id只供ToolPlugin内部路由。
BuiltinToolPlugin不读取或修改Agent可见性；AgentPlugin根据初始化和热插拔操作决定注册哪些资源。
BuiltinToolPlugin的成功响应只表示候选ResourceMapEntry构造完成；MCLPlugin收到并校验回执后原子写入AgentResourceMap和IMPORT绑定，后续INJECT命令决定可见性。
BuiltinToolPlugin只组合内建执行器，不拥有PendingToolCalls、不构造AgentMessage、不执行Inference。
tool:builtin/*不能作为可见资源注册，防止模型绕过资源定义直接调用执行器。
```
