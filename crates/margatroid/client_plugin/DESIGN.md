# 伪代码格式

```text
板块：一级标题，按职责列出core与plugin文件，不创建空模块。
类型、函数：二级标题，区分公开、crate公开与私有。
TypeName：中文名称，可见性类型--用途
    field: Type--字段说明
    method(...) -> ReturnType
        行为：直接描述操作及错误
System：明确处理事件、产出事件，只收集事件并路由到Handler。
属性：不写Rust Attribute。
持有关系：文末使用树形图。
```

# 板块格式

```text
# lib        client/core/src/lib.rs；client/plugin/src/lib.rs  WorldClientExt与公开导出；Plugin安装
# components client/core/src/components.rs                    Client组件
# events     client/core/src/events.rs                        注册结果事件
# system     client/plugin/src/system.rs                      注册、断开事件路由
# handler    client/plugin/src/handler.rs                     注册与断开处理
# types pub  client/core/src/types.rs                         RegisteredClient
# error      client/core/src/error.rs                         ClientError
```

# lib

## 类型

公开，定义于client_core：
```text
WorldClientExt：客户端查询扩展，公开trait
    client_by_connection(&self, connection: WebSocketConnectionId) -> Option<(Entity, &Client)>
        行为：查询connection_id匹配的Client，返回完整Entity和组件借用；不存在返回None。
    client_webui_senders(&self) -> Vec<WebSocketSender>
        行为：查询已注册webui Client；按连接ID去重，返回Server连接表中未关闭的Sender；缺少连接表返回空列表。
    impl WorldClientExt for World
        行为：直接查询Client组件及mecs连接设施，不读取Address。
```

公开，定义于client_plugin：
```text
ClientPlugin：客户端生命周期插件，公开结构体
    schedule: String--私有，System运行计划
    new() -> Self
        行为：使用RuntimePlugin::UPDATE。
    with_schedule(self, schedule: impl Into<String>) -> Self
        行为：替换运行计划。
    impl Default for ClientPlugin
        行为：调用new。
    impl Plugin for ClientPlugin
        build(self, app: &mut App)
            行为：检查计划和WebSocketConnections；拒绝重复安装；写入安装标记；安装注册和断开System。
```

私有，定义于client_plugin：
```text
ClientPluginInstalled：安装标记，私有Resource
    impl Resource for ClientPluginInstalled
```

# components

## 类型

公开：
```text
Client：已注册客户端，公开组件
    connection_id: WebSocketConnectionId--Server连接句柄
    client_type: String--客户端自报类型
    name: String--客户端名称
    impl Component for Client
    impl Clone + Debug for Client
```

# events

## 类型

公开：
```text
ClientRegistrationResult：注册结果，公开事件
    id: String--注册请求ID
    connection_id: WebSocketConnectionId--原连接
    result: Result<RegisteredClient, ClientError>
    impl Event for ClientRegistrationResult
```

# system

## 函数

crate公开：
```text
client_registration_system(world: &mut World)
    处理事件：RegisterConnection
    产出事件：ClientRegistrationResult
    行为：收集事件，逐条调用handle_client_registration。

client_disconnect_system(world: &mut World)
    处理事件：WebSocketDisconnected
    产出事件：无
    行为：收集事件，逐条调用handle_client_disconnect。
```

# handler

## 函数

crate公开：
```text
handle_client_registration(world: &mut World, request: &RegisterConnection)
    行为：获取连接设施，调用register_client；失败记录日志；按原请求ID和连接ID发送ClientRegistrationResult。

handle_client_disconnect(world: &mut World, event: &WebSocketDisconnected)
    行为：销毁connection_id匹配的Client Entity，Address随实体销毁。
```

私有：
```text
register_client(world: &mut World, connections: &WebSocketConnections, request: &RegisterConnection) -> Result<RegisteredClient, ClientError>
    行为：
        校验请求ID、client_type、名称与连接存活状态。
        拒绝已注册连接和重复名称；名称缺省为连接ID文本。
        构造Address：client:<client_type>/<name>:<connection_id>，检查地址未占用。
        保存并更新连接元数据；创建Entity并挂载Client和Address。
        复检连接，成功返回RegisteredClient。
        失败销毁本次Entity并尝试恢复原连接元数据。

valid_client_type(value: &str) -> bool
    行为：要求非空，且只含小写ASCII字母、数字、下划线或短横线。

valid_client_name(value: &str) -> bool
    行为：要求非空，不是点或双点，不含控制字符、斜杠、反斜杠或冒号。
```

# types pub

## 类型

公开：
```text
RegisteredClient：注册成功载荷，公开结构体
    client: Entity--已挂载Client和Address的完整实体句柄
    impl Clone + Debug + PartialEq + Eq for RegisteredClient
```

# error

## 类型

公开：
```text
ClientError：客户端注册错误，公开枚举
    InvalidRequest
    InvalidClientType
    InvalidName
    DuplicateName
    DuplicateClient
    ConnectionMissing
    AddressConflict
    impl Clone + Debug + PartialEq + Eq for ClientError
    impl Display for ClientError
        行为：输出固定错误描述，不回显客户端敏感输入。
    impl std::error::Error for ClientError
```

# 持有关系

```text
App
└── World
    ├── ClientPluginInstalled
    ├── WebSocketConnections（Server管理连接生命周期）
    └── Client Entity
        ├── Address（address提供，plugin挂载）
        └── Client（client_core提供）
            ├── connection_id
            ├── client_type
            └── name

ClientRegistrationResult
└── Result<RegisteredClient, ClientError>
    └── RegisteredClient
        └── Entity（关联句柄，不拥有实体）

调用方
└── Vec<WebSocketSender>（查询时刻的发送器句柄快照）
```
