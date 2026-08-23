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

# 板块格式

每个 crate 的 DESIGN.md 在伪代码格式之后使用六个一级标题，按 `lib`、`system`、`handler`、`events`、`types`、`error` 顺序组织。每个标题对应 `src/` 下的同名 Rust 文件：

```text
# lib        src/lib.rs        图书馆组件与 Plugin
# system     src/system.rs     System 函数
# handler    src/handler.rs    处理函数
# events     src/events.rs     事件类型
# types      src/types.rs      其余类型
# error      src/error.rs      Error 与公开错误分类
```

# lib

lib 放 Plugin 和安装标记 Resource。

## 类型

公开：
```text
ConnectionPlugin：连接元数据插件，公开结构体--根据客户端注册请求维护WebSocket连接类型和名称
    schedule: String--连接注册System所在的Runtime schedule，私有
    new() -> Self
        构造连接插件：公开关联函数，使用RuntimePlugin::UPDATE
    with_schedule(mut self, schedule: impl Into<String>) -> Self
        设置Schedule：公开构建方法
    impl Default for ConnectionPlugin
        Default：公开trait实现，调用new
    impl Plugin for ConnectionPlugin
        Plugin：公开trait实现
        build(self, app: &mut App)
            安装插件：公开trait方法
            行为：
                重复安装时panic
                Schedule不存在时panic
                ServerPlugin未安装WebSocketConnections时panic
                插入ConnectionPluginInstalled
                在schedule挂载connection_registration_system
```

私有：
```text
ConnectionPluginInstalled：ConnectionPlugin安装标记，私有Resource
    impl Resource for ConnectionPluginInstalled
```

# system

system 放 System 函数。System 只读取本帧领域事件并调用 handler；不展开业务逻辑。

## 函数

crate公开：
```text
connection_registration_system(world: &mut World)
    连接注册System：crate公开System
    处理事件：RegisterConnection
    行为：
        克隆本帧全部RegisterConnection
        读取并克隆WebSocketConnections；不存在时直接返回
        逐个调用handle_register_connection
```

# handler

handler 放处理函数。

## 函数

crate公开：
```text
handle_register_connection(connections: &WebSocketConnections, request: &RegisterConnection)
    处理连接注册：crate公开函数
    行为：
        去除client_type首尾空白
        校验client_type非空且只包含小写ASCII字母、数字、下划线和短横线；非法时写warn日志并结束
        生成名称：{client_type}-{connection_id.get()}
        调用set_connection_type写入类型；连接不存在时写warn日志并结束
        调用set_name写入名称；命名失败时写warn日志
        成功时写info日志，包含request_id、connection_id、client_type和name
```

私有：
```text
valid_client_type(value: &str) -> bool
    验证客户端类型：私有函数，要求非空且只包含小写ASCII字母、数字、下划线和短横线
```

# events

```text
ConnectionPlugin 不定义 ECS 事件；它消费 server_plugin 的 RegisterConnection 事件。
```

# types

```text
ConnectionPlugin 不定义其他类型。
```

# error

```text
ConnectionPlugin 不定义错误类型；注册失败只写warn日志，不向调用方返回错误。
```

## 逻辑

```text
客户端连接注册：
    connection.register
        -> DtoPlugin::dto_route_system
        -> RegisterConnection
        -> ConnectionPlugin::connection_registration_system
        -> handle_register_connection
        -> WebSocketConnections::set_connection_type
        -> WebSocketConnections::set_name

连接筛选：
    WebSocketMessageTarget::Broadcast
        -> WebSocketConnections::get_all
    WebSocketMessageTarget::Type(client_type)
        -> WebSocketConnections::get_by_type
    WebSocketMessageTarget::Name(name)
        -> WebSocketConnections::get_by_name
```

## 边界

```text
ConnectionPlugin负责：
    校验客户端连接类型
    为连接生成后端名称
    将类型和名称写入WebSocketConnections

ConnectionPlugin不负责：
    解析WebSocket frame或ClientMessage JSON
    构造Workspace、Agent或ServerMessage
    发送WebSocket消息
    认证客户端或提供权限控制
    处理连接断开；连接生命周期和索引清理由ServerPlugin负责
```
