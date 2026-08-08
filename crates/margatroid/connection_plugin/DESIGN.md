# ConnectionPlugin

## 类型

公开：
```text
ConnectionPlugin：连接元数据插件，公开结构体--根据客户端注册请求维护WebSocket连接类型和名称
    schedule: String--连接注册system所在的Runtime schedule，默认RuntimePlugin::UPDATE
    new() -> Self
        构造连接插件：公开方法
        行为：使用RuntimePlugin::UPDATE构造插件
    with_schedule(schedule: impl Into<String>) -> Self
        设置执行schedule：公开方法
        行为：保存调用者提供的schedule名称并返回自身
    impl Default for ConnectionPlugin
        Default：公开trait实现
        default() -> Self
            构造默认连接插件：使用new返回默认配置
    impl Plugin for ConnectionPlugin
        Plugin：公开trait实现
        build(self, app: &mut App)
            安装连接插件：注册连接注册system
            行为：
                检查ConnectionPlugin没有重复安装
                检查schedule存在
                将connection_registration_system加入schedule
```

私有：
```text
ConnectionPluginInstalled：连接插件安装标记，私有资源--防止同一个App重复安装ConnectionPlugin
```

## 函数

私有：
```text
connection_registration_system(world: &mut World)
    注册连接元数据：私有system，消费ConnectionRegisterRequested并更新ServerPlugin连接注册表
    行为：
        收集当前事件队列中的全部ConnectionRegisterRequested
        逐条处理，不因单条请求失败而终止本帧其他请求
        去除client_type首尾空白
        检查client_type非空且只包含小写ASCII字母、数字、下划线和短横线
        非法类型时写warning日志并结束当前请求
        使用client_type和connection_id生成名称
            名称格式为{client_type}-{connection_id.get()}
        调用WebSocketConnections::set_connection_type写入连接类型
        连接不存在时写warning日志并结束当前请求
        调用WebSocketConnections::set_name写入生成的唯一名称
        命名失败时写warning日志
```

## 逻辑

```text
客户端连接注册：
    connection.register
        -> ApiPlugin::api_route_system
        -> ConnectionRegisterRequested
        -> ConnectionPlugin::connection_registration_system
        -> WebSocketConnections::set_connection_type
        -> WebSocketConnections::set_name
```

```text
连接筛选：
    WebSocketMessageSend::Broadcast
        -> WebSocketConnections::get_all
    WebSocketMessageSend::Type(client_type)
        -> WebSocketConnections::get_by_type
    WebSocketMessageSend::Name(name)
        -> WebSocketConnections::get_by_name
```

## 边界

```text
ConnectionPlugin负责：
    校验客户端连接类型
    为连接生成后端名称
    将类型和名称写入WebSocketConnections

ConnectionPlugin不负责：
    解析WebSocket frame或ClientRequest JSON
    构造Workspace、Agent或ServerEvent
    发送WebSocket消息
    认证客户端或提供权限控制
    处理连接断开；连接生命周期和索引清理由ServerPlugin负责
```

