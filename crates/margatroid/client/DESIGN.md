Margatroid领域实现契约；本领域已切换，mecs现有实现保持不动，后续另行清理。

# 伪代码格式

```text
板块：一级标题，按职责列出core与plugin文件，不创建空模块。
类型、函数：二级标题，区分公开、crate公开与私有。
类型列字段、签名及直接行为；System列处理事件、闭包结果、提交闭包及产出事件。
属性不写Rust Attribute；持有关系使用树形图。
业务plugin之间不建立依赖；core仅暴露领域数据与能力。
```

# 板块格式

```text
# lib        client/core/src/lib.rs；client/plugin/src/lib.rs  查询扩展、导出与安装
# components client/core/src/components.rs                    已注册Client
# events     client/core/src/events.rs                        注册和断连领域事件
# system     client/plugin/src/system.rs                      事件路由
# handler    client/plugin/src/handler.rs                     身份注册与销毁
# types pub  client/core/src/types.rs                         连接标识及发送能力
# error      client/core/src/error.rs                         ClientError与ClientSendError
```

# lib

## 类型

公开，定义于client_core：
```text
WorldClientExt：客户端查询扩展，公开trait
    client_by_connection(&self, connection: ConnectionId) -> Option<(Entity, &Client)>
        行为：查询connection_id匹配的Client，返回完整Entity及组件借用。
    client_webui_senders(&self) -> Vec<ClientSender>
        行为：从已注册webui Client组件克隆未关闭的sender，按ConnectionId去重；返回查询时刻的能力快照。
    impl WorldClientExt for World
        行为：只查询Client组件，不查询传输连接表或Address；inference使用此发送器快照。
```

公开，定义于client_plugin：
```text
ClientPlugin：客户端身份插件
    schedule: String--私有，默认RuntimePlugin::UPDATE
    new() -> Self
    with_schedule(self, schedule: impl Into<String>) -> Self
    impl Default for ClientPlugin
    impl Plugin for ClientPlugin
        build(self, app: &mut App)
            行为：检查运行计划，拒绝重复安装，写安装标记，安装注册及断连System；无需传输设施。
```

私有，定义于client_plugin：
```text
ClientPluginInstalled：安装标记，Resource
```

# components

## 类型

公开：
```text
Client：已注册客户端，组件
    connection_id: ConnectionId--传输中立的连接身份
    client_type: String--客户端自报类型
    name: String--客户端名称
    sender: ClientSender--注册时注入的出站能力
    impl Component + Clone + Debug
```

# events

## 类型

公开，定义于client_core：
```text
RegisterClient：身份注册请求，事件
    id: String--内部注册请求ID，DTO负责关联网络ID
    connection_id: ConnectionId
    client_type: String
    name: Option<String>
    sender: ClientSender--DTO为真实连接创建，客户端不能提交或替换
    impl Event + Clone + Debug

ClientRegistrationResult：注册结果，事件
    id: String
    connection_id: ConnectionId
    result: Result<RegisteredClient, ClientError>
    impl Event + Clone + Debug

ClientDisconnected：连接结束，专用领域事件
    connection_id: ConnectionId
    impl Event + Clone + Debug
    行为：由DTO在连接读写任务终结时恰好发送一次，包括从未注册的连接；其他传输实现遵守相同契约。
```

# system

## 函数

crate公开：
```text
client_registration_system(world: &mut World)
    处理事件：RegisterClient
    接收闭包结果：None
    提交闭包：None
    产出事件：ClientRegistrationResult
    行为：收集事件，逐条调用handle_client_registration。

client_disconnect_system(world: &mut World)
    处理事件：ClientDisconnected
    接收闭包结果：None
    提交闭包：None
    产出事件：None
    行为：收集事件，逐条调用handle_client_disconnect。
```

# handler

## 函数

crate公开：
```text
handle_client_registration(world: &mut World, request: &RegisterClient)
    行为：调用register_client，按原id与connection_id发送ClientRegistrationResult。

handle_client_disconnect(world: &mut World, event: &ClientDisconnected)
    行为：销毁connection_id匹配的Client Entity及Address；重复通知无操作，不撤销已受理业务操作。
```

私有：
```text
register_client(world: &mut World, request: &RegisterClient) -> Result<RegisteredClient, ClientError>
    行为：
        校验请求ID、client_type、名称及sender未关闭；DTO保证连接ID和sender来自同一真实连接。
        拒绝已注册连接和重复名称；名称缺省为连接ID文本。
        构造Address：client:<client_type>/<name>:<connection_id>，检查地址未占用。
        创建Entity，挂载Client（含sender克隆）和Address；复检sender，关闭则销毁本次Entity并失败。
        不修改传输元数据；读端结束时DTO关闭接收端，注册与断连竞态通过sender关闭检查和ClientDisconnected收敛。

valid_client_type(value: &str) -> bool
    行为：非空，只含小写ASCII字母、数字、下划线或短横线。

valid_client_name(value: &str) -> bool
    行为：非空，不是点或双点，不含控制字符、斜杠、反斜杠或冒号。
```

# types pub

## 类型

公开，定义于client_core：
```text
ConnectionId：传输中立连接标识，newtype(u64)
    new(value: u64) -> Self
    get(self) -> u64
    impl Copy + Clone + Debug + PartialEq + Eq + Hash + Ord + Display
    行为：DTO在进程生命周期内分配不复用的ID；没有WebSocket或Axum类型。

ClientFrame：应用出站帧，枚举
    Text(String)--已经序列化的文本，包括JSON文本；不含框架消息类型
    Binary(Vec<u8>)--owned二进制数据
    Close { code: u16, reason: String }--通用关闭码及owned原因文本，由传输实现校验并映射关闭握手
    impl Clone + Debug

ClientSender：出站能力，结构体
    sender: tokio::sync::mpsc::Sender<ClientFrame>--私有，有界队列发送端
    new(sender: tokio::sync::mpsc::Sender<ClientFrame>) -> Self
    try_send(&self, frame: ClientFrame) -> Result<(), ClientSendError>
        行为：仅尝试入队；Full或Closed立即返回，不创建异步任务，不宣称已送达对端。
    is_closed(&self) -> bool
        行为：查询接收端是否结束。
    impl Clone + Debug

RegisteredClient：注册成功载荷
    client: Entity--已挂载Client与Address的完整句柄
    impl Clone + Debug + PartialEq + Eq
```

# error

## 类型

公开：
```text
ClientError：注册错误，枚举
    InvalidRequest | InvalidClientType | InvalidName | DuplicateName
    DuplicateClient | ConnectionMissing | AddressConflict
    impl Clone + Debug + PartialEq + Eq + Display + std::error::Error
    行为：输出固定错误描述，不回显敏感输入。

ClientSendError：入队错误，枚举
    Full | Closed
    impl Clone + Debug + PartialEq + Eq + Display + std::error::Error
```

# 持有关系

```text
World
├── ClientPluginInstalled
└── Client Entity
    ├── Address
    └── Client
        ├── ConnectionId
        ├── client_type、name
        └── ClientSender -> 有界队列发送端

DTO owned ConnState（含未注册连接）
└── ConnectionId -> 出站队列与连接生命周期
    └── 接收端由DTO连接任务拥有

inference
└── WorldClientExt::client_webui_senders() -> Vec<ClientSender>（能力快照）

client_core
├── core组件与事件
└── tokio mpsc（无DTO、axum_plugin或server_plugin依赖）
```
