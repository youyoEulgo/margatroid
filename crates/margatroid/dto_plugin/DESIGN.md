# 伪代码格式

```text
板块：一级标题，一行注明实际文件，不创建空模块。
类型、函数：二级标题，区分公开、crate公开与私有。
类型列出字段及用途；函数列出参数、结果和直接行为。
System明确处理事件、产出事件，只收集事件并路由至Handler。
属性不写Rust Attribute；持有关系使用树形图。
```

# 板块格式

```text
# lib     src/lib.rs      Plugin安装与公开入口
# system  src/system.rs   网络、领域结果及出站事件路由
# handler src/handler.rs  身份绑定、寻址、领域转换与收发
# types   src/types.rs    请求关联、历史结果与投影缓存
```

# lib

## 类型

公开：
```text
DtoPlugin：网络边界插件，公开类型
    impl Plugin for DtoPlugin
        build(self, app: &mut App)
            行为：检查运行、连接和出站配置设施；初始化关联表与投影缓存；安装System及网络事件路由。
```

# system

## 函数

crate公开：
```text
dto_route_system(world: &mut World)
    处理事件：WebSocketMessage、WebSocketDisconnected、MclCommandResult及待发送消息
    产出事件：注册、MCL、Workspace启停与重启、历史查询、Driver查询和目录浏览的领域请求
    行为：收集事件，分别路由到入站、结果、断开及出站Handler。

collect_external_events_system(world: &mut World)
    处理事件：Server生命周期、Client注册结果、Workspace结果、历史结果、Driver清单、目录浏览结果及状态变化
    产出事件：无新增领域请求；准备协议回执和主动出站消息
    行为：收集事件，分别路由到结果和状态投影Handler。
```

# handler

## 函数

crate公开：
```text
handle_inbound_message(world: &mut World, connection: WebSocketConnectionId, message: WebSocketMessage)
    行为：
        检查消息上限并按protocol解析信封。
        除注册外，从真实连接查询Client Entity；拒绝未注册连接及重复在途网络ID。
        运行目标经address解析为Entity，核对组件、Workspace归属及实例。
        Workspace配置项同步从启动时加载的内存配置读取；文件资源使用定位字符串，不创建占位Entity。
        列出Workspace直接投影内存配置并回复，不发领域事件或提交异步任务。
        回合中止同步调用Agent回合扩展及执行取消能力，直接返回协议受理结果，不发送Agent中止事件。
        其他异步请求生成operation_id，保存网络关联，转换为领域请求并发送；失败返回关联协议错误。

handle_domain_result(world: &mut World, result: DomainResult)
    行为：
        按operation_id取请求关联，核对操作类型、原目标Entity及实例。
        复检原Client Entity和连接；转换为protocol响应并只发原连接。
        完成后释放关联；重复、迟到或已断连结果不重新绑定目标。

handle_disconnect(world: &mut World, event: &WebSocketDisconnected)
    行为：清理该连接的请求关联和发送状态，不撤销已受理领域操作。

prepare_backend_state(world: &World) -> Result<BackendStateDto, ProtocolErrorDto>
    行为：读取领域core公开组件与已加载内存配置；从Address取得对外地址；生成不含凭据、源码及内部句柄的协议投影。

handle_outbound_message(world: &mut World, message: PreparedMessage)
    行为：按目标连接及出站配置发送protocol消息。
```

私有：
```text
resolve_live_target(world: &World, address: &str) -> Result<Entity, ProtocolErrorDto>
    行为：通过address解析唯一Entity；检查所需领域组件、归属和实例。

convert_request(message: ClientMessage, context: RequestContext) -> Result<DomainRequest, ProtocolErrorDto>
    行为：将纯协议值转换为领域core事件；内存对象关联只使用Entity。

convert_result(result: DomainResult, context: RequestContext) -> Result<ServerMessage, ProtocolErrorDto>
    行为：映射领域结果、错误及关联信息；需要对外地址时从Address读取。
```

# types

## 类型

私有：
```text
RequestContext：边界请求上下文
    network_id: String--原网络请求ID
    operation_id: String--内部唯一操作ID
    client: Entity--原Client完整实例
    connection: WebSocketConnectionId--原连接
    target: OperationTarget--操作目标

OperationTarget：目标关联，私有枚举
    Live { workspace: Option<Entity>, target: Entity, instance: Option<String> }
    Discovery
    Registration

PendingRequests：在途关联表
    entries: BTreeMap<String, RequestContext>--operation_id为键
    duplicate(&self, client: Entity, network_id: &str) -> bool
        行为：检查同一Client尚未结束的网络ID。
    remove_connection(&mut self, connection: WebSocketConnectionId)
        行为：删除连接关联，不触发业务取消。

DomainRequest：领域请求，私有枚举--包装对应core的注册、生命周期、MCL及查询请求
DomainResult：领域结果，私有枚举--包装对应core结果；各结果保留operation_id
PreparedMessage：待发送消息，私有类型--保存发送目标和ServerMessage
BackendStateReportCache：投影缓存，私有类型--缓存最近出站投影及接收者，不保存第二份领域运行状态
```

# 持有关系

```text
App
└── World
    ├── PendingRequests
    │   └── RequestContext
    │       ├── Client Entity（关联）
    │       ├── connection_id
    │       └── OperationTarget（关联）
    └── BackendStateReportCache

DtoPlugin
├── protocol（纯网络类型）
├── address（边界寻址）
├── 各领域core（组件、请求及结果）
└── mecs（连接、事件及异步设施）
```
