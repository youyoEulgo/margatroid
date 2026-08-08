# 伪代码格式
```text
模块：使用一级标题，只写当前设计涉及的部分

类型：使用二级标题，按私有、crate公开和公开分组
TypeName：中文类型名，可见性类型--类型说明
    field_name: RustType--中文字段名，字段说明
    method_name<Generic>(self, parameter: ParameterType) -> ReturnType
        中文方法名：可见性方法，解释参数和用途
        约束：使用标准Rust where约束；单个约束写在同一行
        行为：展开完整逻辑
    impl TraitName for TypeName
        TraitName：可见性trait实现
        trait_method_name(&self, parameter: ParameterType) -> ReturnType
            中文方法名：解释参数和用途
            行为：标准库trait的简单行为用一句话说明

函数：使用二级标题，按私有和公开分组，只放置不属于某个类型的操作
function_name<Generic>(parameter: ParameterType) -> ReturnType
    中文函数名：可见性函数，解释参数和用途
    约束：使用标准Rust where约束；单个约束写在同一行
    行为：展开完整逻辑

逻辑：使用二级标题，按执行顺序描述对象之间的调用关系
注释：字段注释使用--，类型、方法和函数的说明直接写在标题中
属性：不写Rust Attribute，实现时自行判断
边界：对外使用泛型和具体类型，内部使用类型擦除
```

# ApiIntegrationPlugin

## 类型

公开：
```text
ApiIntegrationPlugin：API应用层适配插件，公开结构体--连接ApiPlugin的传输事件与Margatroid领域事件和状态
    schedule: String--全部同步System所属Schedule，默认RuntimePlugin::UPDATE
    frontend_type: String--接收完整状态快照的连接类型，默认webui
    new() -> Self
        构造插件：公开关联函数，使用默认Schedule和前端类型
    with_schedule(self, schedule: impl Into<String>) -> Self
        设置Schedule：公开方法，替换同步System所属Schedule并返回自身
    with_frontend_type(self, frontend_type: impl Into<String>) -> Self
        设置前端类型：公开方法，替换状态快照的连接类型并返回自身
    impl Default for ApiIntegrationPlugin
        Default：公开trait实现
        default() -> Self
            构造默认插件：调用new
    impl Plugin for ApiIntegrationPlugin
        Plugin：公开trait实现
        build(self, app: &mut App)
            安装适配器：检查安装条件，启动日志转发服务并注册请求、报告和快照System
```

私有：
```text
ApiIntegrationPluginInstalled：插件安装标记，私有资源--阻止重复安装
```

## 函数

私有：
```text
forward_logs(stream: TracingStream, events: RuntimeEventSender)
    转发结构化日志：私有异步函数，持续将TracingRecord转换为广播WebSocketMessageSend

log_record(record: TracingRecord) -> LogRecordDto
    转换日志：私有函数，逐字段构造协议日志DTO

handle_workspace_start_requests(world: &mut World)
    处理Workspace请求：私有System，将WorkspaceDefinitionDto恢复为领域定义并发送StartWorkspace

handle_agent_message_requests(world: &mut World)
    处理Agent消息请求：私有System，解析逻辑Workspace和Agent后发送用户AgentMessage

report_runtime_events(world: &mut World)
    投影运行事件：私有System，依次调用各类运行事件报告函数

report_server_events(world: &World)
    报告Server事件：私有函数，将Server生命周期事件写入结构化日志

report_workspace_events(world: &World)
    报告Workspace事件：私有函数，将启动结果转换为日志和WebSocketMessageSend

report_agent_messages(world: &World)
    报告Agent消息：私有函数，反查Agent逻辑身份并转换为WebSocketMessageSend

report_agent_failures(world: &World)
    报告Agent失败：私有函数，反查Agent逻辑身份并转换为日志和WebSocketMessageSend

sync_frontend_state_system(world: &mut World, frontend_type: &str)
    同步完整状态：私有System，构造BackendStateDto并发送给指定连接类型

backend_state(world: &World) -> Result<BackendStateDto, String>
    构造后端状态：私有函数，按Workspace读取Agent历史表并生成完整有序快照

route_agent_message(world: &World, id: String, workspace: WorkspaceRefDto, agent: Option<String>, content: String) -> Result<(), String>
    路由用户消息：私有函数，校验输入，按逻辑身份查找显式Agent或manager并发送AgentMessage

workspace_info(world: &World, workspace: Entity) -> Option<WorkspaceInfoDto>
    转换Workspace：私有函数，从WorkspaceConfiguration构造协议DTO

agent_route(world: &World, agent: Entity) -> Option<(WorkspaceRefDto, String)>
    反查Agent身份：私有函数，从Workspace索引取得Agent逻辑名称和Workspace引用
```

## 逻辑

```text
入站业务请求：
    ApiPlugin发送WorkspaceStartRequested或AgentMessageRequested
        -> ApiIntegrationPlugin解析协议DTO和逻辑身份
        -> 发送StartWorkspace或AgentMessage领域事件

出站业务状态：
    Workspace、Agent、Memory和Server产生领域事件或状态
        -> ApiIntegrationPlugin构造ServerEvent
        -> 发送WebSocketMessageSend
        -> ApiPlugin序列化并选择连接

日志：
    LogPlugin::TracingStream
        -> forward_logs异步订阅
        -> WebSocketMessageSend::Broadcast

边界：
    ApiIntegrationPlugin不解析或序列化WebSocket JSON，不持有发送器，不执行领域业务
    ApiPlugin不查询Workspace、Agent或Memory，不构造业务DTO
    Workspace、Agent和Memory Plugin不依赖API协议或客户端连接
    daemon不注册业务System，只配置并安装Plugin
```
