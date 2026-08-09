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
ApiIntegrationPlugin：前端状态与日志集成插件，公开结构体--生成后端完整状态并转发结构化日志
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
            安装集成：检查安装条件，启动日志转发服务并注册Server日志和状态快照System
```

私有：
```text
ApiIntegrationPluginInstalled：插件安装标记，私有资源--阻止重复安装
```

## 函数

私有：
```text
forward_logs(stream: TracingStream, events: RuntimeEventSender)
    转发结构化日志：私有异步函数，调用TracingRecord::into_dto并发送广播WebSocketMessageSend

report_server_events(world: &mut World)
    报告Server事件：私有函数，将Server生命周期事件写入结构化日志

sync_frontend_state_system(world: &mut World, frontend_type: &str)
    同步完整状态：私有System，调用()::into_dto(&World)构造BackendStateDto并发送给指定连接类型

```

## 逻辑

```text
完整状态：
    Workspace、Agent和Memory当前状态
        -> Protocol的BackendStateDto::from_domain读取完整状态
        -> sync_frontend_state_system包装ServerMessage
        -> WebSocketMessageSend::Type(frontend_type)

日志：
    LogPlugin::TracingStream
        -> forward_logs异步订阅
        -> WebSocketMessageSend::Broadcast

边界：
    ApiIntegrationPlugin不解析或序列化WebSocket JSON，不持有发送器，不执行领域业务
    DtoPlugin负责DTO转换、入站领域事件发送和即时出站事件投影
    Workspace、Agent和Memory Plugin不依赖API协议或客户端连接
    daemon不注册业务System，只配置并安装Plugin
```
