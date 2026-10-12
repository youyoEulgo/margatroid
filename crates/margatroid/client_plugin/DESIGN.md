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

本文为目标设计，尚未实现。统一Client共享组件、注册握手和推流快照契约，不延续独立ResourceId组件。

# lib

## 类型

公开，组件与World扩展声明和实现均位于共享client_components.rs：
```text
Client：已注册客户端，公开图书馆组件
    resource_id: ResourceID--client:<type>/<name>:<connection_id>
    connection_id: WebSocketConnectionId--服务端连接句柄
    client_type: String--注册类型，webui用于推理发送器筛选
    name: String--注册名称，缺省连接ID文本
    impl Component for Client
    impl LibraryComponent for Client
        resource_id(&self) -> &ResourceID
            读取身份：公开trait方法
            行为：借用内部resource_id。

WorldClientExt：客户端查询，公开trait
    client_by_connection(&self, connection: WebSocketConnectionId) -> Option<&Client>
        查询注册客户端：公开方法
        行为：遍历Client匹配连接ID，不存在返回None。
    client_webui_senders(&self) -> Vec<WebSocketSender>
        收集webui发送器：公开方法
        行为：遍历已注册webui Client，通过Server共享连接接口取得Sender，按连接ID去重并克隆；缺失或断开忽略，无客户端为空。
    impl WorldClientExt for World
        World：公开trait实现，读取共享Client与Server连接契约，不依赖ClientPlugin Handler。

ClientPlugin：注册生命周期插件，公开结构体
    schedule: String
    impl Plugin for ClientPlugin
        build(self, app: &mut App)
            安装插件：公开trait方法
            行为：检查计划、连接查询设施及统一资源插件，注册LibraryComponent类型client，挂载注册与断开System，拒绝重复安装。
```

## 逻辑

```text
Server拥有连接和发送器生命周期，Client只保存已注册身份与连接句柄
Entity只挂Client图书馆组件，不挂独立ResourceId
client_type及name是自报显示与路由数据，不是权限凭据；source由入口依据真实连接的已注册身份绑定
推理在提交闭包前调用client_webui_senders取得全部快照，不按Workspace隔离，后来连接不补入
发送器关闭或慢客户端由本次推流处理，无发送器不影响推理oneshot结果
```

# system

## 函数

crate公开：
```text
client_registration_system(world: &mut World)
    注册System：crate公开System
    处理事件：RegisterConnection
    产出事件：ClientRegistrationResult
    行为：克隆本帧注册请求，逐个调用handle_client_registration，成功失败均发关联回执。

client_disconnect_system(world: &mut World)
    断开System：crate公开System
    处理事件：WebSocketDisconnected
    行为：逐个处理断开连接，销毁对应Client，不替换已有推理闭包Sender快照。
```

# handler

## 函数

crate公开：
```text
handle_client_registration(world: &mut World, request: RegisterConnection)
    处理注册握手：crate公开函数
    行为：
        校验请求ID、连接存在、client_type为非空小写ASCII字母数字及下划线或短横线
        名称缺省连接ID文本，构造完整ResourceID并检查地址与名称冲突
        更新连接元数据，通过统一受检图书馆入口创建并挂Client
        准备失败不留下半注册实体；更新连接元数据失败返回关联错误
        只有Client实体可查询后才发送注册成功，客户端随后才能发依赖身份的MCL请求

handle_client_disconnect(world: &mut World, connection: WebSocketConnectionId)
    处理断开：crate公开函数
    行为：销毁该连接Client，未注册连接无操作；连接关闭由Server处理，不取消已提交推理。
```

# events

## 类型

公开，全部事件归共享client_events.rs，Server输入归共享server_events.rs：
```text
ClientRegistrationResult：注册结果，公开事件
    id: String
    connection_id: WebSocketConnectionId
    result: Result<RegisteredClient, ClientError>
    impl Event for ClientRegistrationResult

RegisteredClient：注册成功载荷，公开共享结构体
    client: Entity
    resource_id: ResourceID
```

# types

## 逻辑

```text
MCL网络入口必须取得已注册Client ResourceID作为source；拒绝unregistered或客户端自行覆盖source
Driver宿主以真实Agent身份绑定source，不使用目标参数充当来源
入口将text及arguments数组列表转换为MclInstruction，没有binding隐式单参数兼容
WebSocket增量编码沿用客户端消息协议，关联请求ID与Agent，oneshot才是推理权威结果
```

# error

## 类型

公开共享契约：
```text
ClientError：注册错误，公开枚举
    InvalidRequest
    InvalidClientType
    InvalidResourceID
    DuplicateName
    DuplicateClient
    ConnectionMissing
    ResourceSetupFailed
    impl Clone + Debug + PartialEq + Eq for ClientError
    impl fmt::Display for ClientError
        fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result
            格式化错误：公开trait方法
            行为：输出稳定错误分类，不回显敏感客户端输入。
    impl std::error::Error for ClientError
```

## 逻辑

```text
注册失败必须有回执，不只写warning；Source绑定保证来源对应真实连接，不等同于身份认证或完整权限系统。
```
