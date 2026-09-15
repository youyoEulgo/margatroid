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
# events     src/events.rs     事件类型（当前无）
# types      src/types.rs      其余类型（当前无）
# error      src/error.rs      Error 与公开错误分类
```

# lib

lib 只放图书馆组件和 Plugin。

图书馆组件是 Entity 必须挂载的领域组件，组件存在本身表明 Entity 的领域身份；例如：

```text
Client Entity 必须挂载 Client 组件 + ResourceId 组件
```

ClientPlugin 结构体，以及 ClientPluginInstalled 安装标记 Resource，也放在 lib。

## 类型

公开：
```text
Client：客户端图书馆组件，公开 Component--Client Entity 必须挂载 Client 和 ResourceId；组件存在本身表明 Entity 是一个已注册客户端
    connection_id: WebSocketConnectionId--服务端分配的连接句柄
    client_type: String--注册时客户端自报的连接类型
    name: String--注册时客户端提供的名称；未提供时等于连接句柄的十进制文本
    connection_id(&self) -> WebSocketConnectionId
        读取连接句柄：公开方法
    client_type(&self) -> &str
        读取连接类型：公开方法
    name(&self) -> &str
        读取名称：公开方法
    impl Component for Client

ClientPlugin：客户端插件，公开结构体--把注册成功的客户端实体化，并在连接断开时清除
    schedule: String--注册与断开System所在的Runtime schedule，私有
    new() -> Self
        构造客户端插件：公开关联函数，使用RuntimePlugin::UPDATE
    with_schedule(mut self, schedule: impl Into<String>) -> Self
        设置Schedule：公开构建方法
    impl Default for ClientPlugin
        Default：公开trait实现，调用new
    impl Plugin for ClientPlugin
        Plugin：公开trait实现
        build(self, app: &mut App)
            安装插件：要求RuntimePlugin、ResourceIdPlugin和ServerPlugin已安装
            行为：
                重复安装时panic
                Schedule不存在时panic
                ServerPlugin未安装WebSocketConnections时panic
                ResourceIdPlugin未安装时panic
                插入ClientPluginInstalled
                在schedule依次挂载client_registration_system和client_disconnect_system
```

私有：
```text
ClientPluginInstalled：ClientPlugin安装标记，私有Resource
    impl Resource for ClientPluginInstalled
```

# system

system 放 System 函数。System 只读取本帧领域事件并调用 handler；不展开业务逻辑。

## 函数

crate公开：
```text
client_registration_system(world: &mut World)
    客户端注册System：crate公开System
    处理事件：RegisterConnection
    行为：
        克隆本帧全部RegisterConnection
        读取并克隆WebSocketConnections；不存在时直接返回
        逐个调用handle_client_registration；返回错误时写warn日志并继续

client_disconnect_system(world: &mut World)
    客户端断开System：crate公开System
    处理事件：WebSocketDisconnected
    行为：
        克隆本帧全部WebSocketDisconnected
        逐个调用handle_client_disconnect
```

# handler

handler 放处理函数。

## 函数

crate公开：
```text
handle_client_registration(world: &mut World, connections: &WebSocketConnections, request: &RegisterConnection) -> Result<Entity, ClientError>
    处理客户端注册：crate公开函数
    行为：
        去除client_type首尾空白
        校验client_type非空且只包含小写ASCII字母、数字、下划线和短横线；非法返回InvalidClientType
        调用client_resource_id构造资源ID；失败返回InvalidResourceId
        该资源ID已有Entity时返回DuplicateClient
        调用set_name写入名称；名称被占用返回DuplicateName，其余失败返回ConnectionMissing
        调用set_connection_type写入类型；连接不存在返回ConnectionMissing
        spawn Entity并挂载ResourceId与Client
        写info日志，包含request_id、connection_id、client_type、name和resource_id
        返回Entity

handle_client_disconnect(world: &mut World, connection_id: WebSocketConnectionId)
    处理客户端断开：crate公开函数
    行为：
        在query_with::<Client>()的结果中查找connection_id匹配的Entity
        命中时despawn该Entity
        未命中时不做任何事
```

私有：
```text
client_resource_id(client_type: &str, name: Option<&str>, connection_id: WebSocketConnectionId) -> Result<ResourceId, ClientError>
    构造客户端资源ID：私有函数，格式为 client:<client_type>/<name>:<connection_id>
    行为：
        name为None时取连接句柄的十进制文本
        调用ResourceId::parse解析；失败返回InvalidResourceId

valid_client_type(value: &str) -> bool
    验证客户端类型：私有函数，要求非空且只包含小写ASCII字母、数字、下划线和短横线
```

# events

```text
ClientPlugin 不定义 ECS 事件；它消费 server_plugin 的 RegisterConnection 和 WebSocketDisconnected 事件。
```

# types

```text
ClientPlugin 不定义其他类型。
```

# error

error 放 Error 类型和公开错误分类。注册失败只写warn日志，不向调用方返回错误。

## 类型

公开：
```text
ClientError：客户端错误，公开枚举--不回显客户端提供的类型与名称
    InvalidClientType--client_type不是稳定标识符
    InvalidResourceId--由类型、名称与连接句柄构造的ResourceId不合法
    DuplicateName--名称已被另一条连接占用
    ConnectionMissing--连接在注册完成前消失
    DuplicateClient--同一资源ID已经存在Client Entity
    impl Clone + Debug + PartialEq + Eq for ClientError
    impl fmt::Display for ClientError
        Display：公开trait实现，输出不含客户端输入值的稳定描述
    impl std::error::Error for ClientError
        Error：公开trait实现
```

## 逻辑

```text
客户端连接注册：
    connection.register
        -> DtoPlugin::dto_route_system
        -> RegisterConnection { id, connection_id, client_type, name }
        -> ClientPlugin::client_registration_system
        -> handle_client_registration
        -> WebSocketConnections::set_name
        -> WebSocketConnections::set_connection_type
        -> spawn Entity + ResourceId + Client

客户端连接断开：
    ServerPlugin 关闭连接并从注册表移除发送器
        -> WebSocketDisconnected { connection_id, reason }
        -> ClientPlugin::client_disconnect_system
        -> handle_client_disconnect
        -> 按 connection_id 找到 Client Entity
        -> World::despawn

连接筛选（沿用既有出站路径，不经过ClientPlugin）：
    WebSocketMessageTarget::Broadcast
        -> WebSocketConnections::get_all
    WebSocketMessageTarget::Type(client_type)
        -> WebSocketConnections::get_by_type
    WebSocketMessageTarget::Name(name)
        -> WebSocketConnections::get_by_name
```

## 边界

```text
ClientPlugin负责：
    校验客户端连接类型
    为连接生成或采用名称
    将类型和名称写入WebSocketConnections
    把注册成功的客户端实体化为 Client Entity
    在连接断开时清除对应的 Client Entity

ClientPlugin不负责：
    解析WebSocket frame或ClientMessage JSON
    WebSocket升级、连接发送器与注册表索引的生命周期
    构造Workspace、Agent或ServerMessage
    发送WebSocket消息
    认证客户端或提供权限控制

ClientPlugin 不消费 WebSocketConnected：client_type 与 name 只在注册消息中到达，连接建立时无法构造资源ID；连接建立后未注册就断开的连接不会产生 Entity。
Client Entity 的身份由服务端分配的 connection_id 决定；client_type 与 name 是客户端自报值，只用于构造可读资源ID与出站路由，不参与任何授权判断。
注册目前没有回执消息，客户端无法直接得知注册是否成功。
```

## 持有关系

```text
App
└── World
    ├── Client Entity--每个注册成功的客户端一个
    │   ├── ResourceId--client:<client_type>/<name>:<connection_id>
    │   └── Client
    │       ├── connection_id
    │       ├── client_type
    │       └── name
    └── ClientPluginInstalled Resource

ServerPlugin
└── WebSocketConnections Resource--持有连接发送器和按ID、类型、名称的索引
    ClientPlugin只写入类型与名称，不持有发送器
```
