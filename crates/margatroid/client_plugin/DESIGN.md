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

```text
# lib        src/lib.rs                                      Plugin、安装逻辑与 World 扩展
# components types/src/client_components.rs                  Client 图书馆组件
# events     types/src/client_events.rs                      注册结果事件
# system     src/system.rs                                   注册与断开 System
# handler    src/handler.rs                                  注册、断开与身份查询
# types      src/types.rs                                    私有辅助定义
# types pub  types/src/client_types.rs                       结果载荷与来源辅助函数
# error      types/src/client_events.rs                      注册错误
```


# lib

## 类型

公开：
```text
ClientPlugin：客户端注册生命周期插件，公开结构体--管理注册与断开 System 的安装
    schedule: String--私有，注册与断开 System 所属计划
    new() -> Self
        创建插件：公开关联函数
        行为：schedule 使用 RuntimePlugin::UPDATE。
    with_schedule(self, schedule: impl Into<String>) -> Self
        指定计划：公开方法
        行为：替换 schedule，返回修改后的插件。
    impl Default for ClientPlugin
        Default：公开trait实现
        default() -> Self
            行为：调用 Self::new()。
    impl Plugin for ClientPlugin
        Plugin：公开trait实现
        build(self, app: &mut App)
            行为：
                若 World 已持有 ClientPluginInstalled，则 panic，拒绝重复安装。
                若指定 schedule 不存在，则 panic。
                若 World 缺少 WebSocketConnections，则 panic，要求先安装 ServerPlugin。
                若 World 缺少 ResourcePluginInstalled，则 panic，要求先安装 ResourcePlugin。
                调用 register_library::<Client>("client")，失败则 panic。
                向 World 插入 ClientPluginInstalled。
                在指定计划依次添加 client_registration_system、client_disconnect_system。

WorldClientExt：客户端查询扩展，公开trait--查询连接身份及 webui 发送器
    client_by_connection(&self, connection: WebSocketConnectionId) -> Option<(Entity, &Client)>
        查询连接身份：公开 trait 方法
    client_webui_senders(&self) -> Vec<WebSocketSender>
        查询已注册 webui 发送器：公开 trait 方法
    impl WorldClientExt for World
        WorldClientExt：公开trait实现
        client_by_connection(&self, connection: WebSocketConnectionId) -> Option<(Entity, &Client)>
            行为：
                遍历挂有 Client 的 Entity，取得组件。
                返回第一个 connection_id 匹配的完整 Entity 及 Client 借用；未找到返回 None。
        client_webui_senders(&self) -> Vec<WebSocketSender>
            行为：
                若 World 不持有 WebSocketConnections，返回空列表。
                创建连接 ID 去重集合。
                遍历挂有 Client 的 Entity：
                    忽略 client_type 不是 webui 的组件。
                    忽略连接 ID 已进入去重集合的组件。
                    将当前连接 ID 加入集合，取得对应 Server Sender。
                    仅收集存在且尚未关闭的 Sender 句柄。
                返回收集结果，不按 Workspace 隔离。
```

私有：
```text
ClientPluginInstalled：插件安装标记，私有结构体--防止重复安装
    impl Resource for ClientPluginInstalled
        Resource：私有类型的资源标记实现
```

# components

## 类型

公开：
```text
Client：已注册客户端，公开图书馆组件--保存注册身份及连接关联
    resource_id: ResourceID--公开，client:<client_type>/<name>:<connection_id>
    connection_id: WebSocketConnectionId--公开，Server 连接句柄
    client_type: String--公开，客户端自报类型
    name: String--公开，客户端名称，缺省为连接 ID 文本
    impl Component for Client
        Component：公开组件标记实现
    impl LibraryComponent for Client
        LibraryComponent：公开trait实现
        resource_id(&self) -> &ResourceID
            行为：借用 self.resource_id。
```

# events

## 类型

公开：
```text
ClientRegistrationResult：客户端注册结果，公开事件--关联原注册请求与处理结果
    id: String--公开，原注册请求 ID
    connection_id: WebSocketConnectionId--公开，原注册连接 ID
    result: Result<RegisteredClient, ClientError>--公开，注册成功载荷或分类错误
    impl Event for ClientRegistrationResult
        Event：公开事件标记实现
```

# system

## 函数

crate公开：
```text
client_registration_system(world: &mut World)
    处理注册事件：crate 公开函数
    处理事件：RegisterConnection
    产出事件：ClientRegistrationResult
    行为：
        收集本帧 RegisterConnection 事件。
        对每条事件调用 handle_client_registration。

client_disconnect_system(world: &mut World)
    处理断开事件：crate 公开函数
    处理事件：WebSocketDisconnected
    产出事件：无
    行为：
        收集本帧 WebSocketDisconnected 事件。
        对每条事件调用 handle_client_disconnect。
```

# handler

## 函数

crate公开：
```text
handle_client_registration(world: &mut World, request: &RegisterConnection)
    处理注册请求：crate 公开函数
    行为：
        从 World 克隆 WebSocketConnections 句柄。
        若连接设施存在，调用 register_client(world, &connections, request) 取得注册结果。
        否则注册结果为 Err(ClientError::ConnectionMissing)。
        若注册失败，记录 warning，包含请求 ID、连接 ID 及错误。
        无论成功失败，发送 ClientRegistrationResult：
            id、connection_id 来自原请求，result 使用注册结果。

handle_client_disconnect(world: &mut World, event: &WebSocketDisconnected)
    处理断开事件：crate 公开函数
    行为：
        从 event.connection_id 取得断开的连接 ID。
        查询全部 Client，收集 connection_id 与该连接 ID 匹配的 Entity。
        销毁收集到的全部 Entity；没有匹配项则无操作。
        不关闭 Server 连接，不取消已提交推理，也不修改已有 Sender 快照。
```

私有：
```text
register_client(world: &mut World, connections: &WebSocketConnections,
                request: &RegisterConnection) -> Result<RegisteredClient, ClientError>
    注册客户端：私有函数
    行为：
        若 request.id 去除首尾空白后为空，返回 InvalidRequest。
        若 valid_client_type(request.client_type) 为 false，返回 InvalidClientType。
        若连接不存在或 Sender 已关闭，返回 ConnectionMissing。
        若 client_by_connection 已找到该连接的 Client，返回 DuplicateClient。
        将连接 ID 转为十进制字符串 fallback。
        name 使用请求名称；未提供时使用 fallback。
        构造 ResourceID("client", request.client_type, name, fallback)。
        构造失败返回 InvalidResourceID。
        查询该 ResourceID 的拥有实体，查询失败返回 ResourceSetupFailed。
        若已存在拥有实体，返回 DuplicateClient。
        遍历全部 Client，若任一 Client.name 等于 name，返回 DuplicateName。
        保存连接原有 name 和 connection_type；缺省使用空字符串。
        设置连接名称：
            名称冲突返回 DuplicateName，其他设置失败返回 ConnectionMissing。
        设置连接类型：
            若失败，尝试恢复旧名称，返回 ConnectionMissing。
        创建 Entity，构造 Client，保存 ResourceID、连接 ID、类型与名称。
        通过 insert_library 将 Client 挂载到 Entity：
            若失败，销毁 Entity，尝试恢复旧名称与旧类型，返回 ResourceSetupFailed。
        再次检查连接是否存在且 Sender 未关闭：
            若检查失败，销毁 Entity，尝试恢复旧名称与旧类型，返回 ConnectionMissing。
        返回 RegisteredClient，包含已挂载 Client 的完整 Entity 与 ResourceID。

```


公开：
```text
registered_client(world: &World, connection_id: WebSocketConnectionId) -> Option<ResourceID>
    查询注册身份：公开函数
    行为：调用 client_by_connection，找到时克隆 Client.resource_id，否则返回 None。

```

私有：
```text
valid_client_type(value: &str) -> bool
    校验客户端类型：私有函数
    行为：仅当字符串非空，且所有字节都是 ASCII 小写字母、数字、下划线或短横线时返回 true。
```

# types

```text
无私有辅助类型或函数，src/types.rs 保留为空。
```

# types pub

## 类型

公开：
```text
RegisteredClient：注册成功载荷，公开结构体--保存已注册实体及资源身份
    client: Entity--公开，已挂载 Client 的完整实体句柄，包含 generation
    resource_id: ResourceID--公开，该 Client 的规范身份
```

## 函数

公开：
```text
client_source(client_type: Option<&str>, name: Option<&str>,
              connection_id: WebSocketConnectionId) -> String
    格式化来源文本：公开函数
    行为：
        若 client_type 与 name 均存在，返回 client:<client_type>/<name>:<connection_id>。
        否则返回 unregistered:<connection_id>。
        此函数仅拼接文本，不校验 ResourceID、注册状态、Entity generation 或连接存活，
        不得用于替代真实注册身份查询。
```

# error

## 类型

公开：
```text
ClientError：客户端注册错误，公开枚举--不回显客户端敏感输入
    InvalidRequest--注册请求 ID 为空
    InvalidClientType--客户端类型不符合稳定标识符约束
    InvalidResourceID--无法构造客户端资源地址
    DuplicateName--客户端名称已占用
    DuplicateClient--连接或资源地址已注册
    ConnectionMissing--注册所需连接不存在或已关闭
    ResourceSetupFailed--资源拥有者查询或图书馆挂载失败
    impl fmt::Display for ClientError
        Display：公开trait实现
        fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result
            行为：按变体向 formatter 写入固定文本，不包含客户端敏感输入：
                InvalidRequest → "client registration request ID is empty"
                InvalidClientType → "client type is not a stable identifier"
                InvalidResourceID → "client resource id could not be built"
                DuplicateName → "client name is already in use"
                DuplicateClient → "client is already registered"
                ConnectionMissing → "connection disappeared before registration"
                ResourceSetupFailed → "client resource setup failed"
    impl std::error::Error for ClientError
        Error：公开trait实现
```

# 持有关系

```text
App
├── ClientPlugin（安装时消费）
│   └── schedule: String
└── World
    ├── ClientPluginInstalled: Resource
    ├── WebSocketConnections（共享连接设施句柄，连接生命周期由 Server 管理）
    │   └── WebSocketSender（连接发送器句柄）
    ├── Entity
    │   └── Client: LibraryComponent
    │       ├── resource_id: ResourceID
    │       ├── connection_id: WebSocketConnectionId（关联连接，不拥有连接）
    │       ├── client_type: String
    │       └── name: String
    └── ClientRegistrationResult: Event
        ├── id: String
        ├── connection_id: WebSocketConnectionId
        └── result: Result<RegisteredClient, ClientError>
            └── RegisteredClient
                ├── client: Entity（完整句柄，不拥有实体）
                └── resource_id: ResourceID

调用方
└── client_webui_senders() 返回的 Vec<WebSocketSender>
    └── 调用时刻的发送器句柄快照（后续注册不补入，连接关闭可见）
```
