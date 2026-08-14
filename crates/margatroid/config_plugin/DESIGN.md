# ConfigPlugin

## 类型

公开：
```text
WebSocketMessageTarget：WebSocket消息目标，公开枚举--保存尚未解析为连接发送器的目标
    Broadcast--全部当前连接
    Type(String)--指定连接类型
    Name(String)--指定连接名称

MargatroidConfig：Margatroid全局配置，公开只读Resource--从主目录config.toml完整加载
    server_bind: SocketAddr--ServerPlugin监听地址
    logs: Vec<WebSocketMessageTarget>--日志、Workspace启停结果及成员失败异常的目标
    backend_state: Vec<WebSocketMessageTarget>--完整后端状态目标
    member_messages: Vec<WebSocketMessageTarget>--完整成员消息目标
    streaming_member_messages: Vec<WebSocketMessageTarget>--流式成员消息目标
    new(server_bind, logs, backend_state, member_messages, streaming_member_messages) -> Result<Self, ConfigError>
        构造配置：公开关联函数，保存监听地址并验证四组目标非空、合法且不重复
    server_bind(&self) -> SocketAddr
    logs(&self) -> &[WebSocketMessageTarget]
    backend_state(&self) -> &[WebSocketMessageTarget]
    member_messages(&self) -> &[WebSocketMessageTarget]
    streaming_member_messages(&self) -> &[WebSocketMessageTarget]
    impl Resource for MargatroidConfig

ConfigPlugin：全局配置插件，公开结构体--读取配置并安装只读Resource
    config: MargatroidConfig--已完整验证的配置，私有
    open(path: impl Into<PathBuf>) -> Result<Self, ConfigError>
        打开配置：公开关联函数，有界读取并解析config.toml
    new(config: MargatroidConfig) -> Self
        使用配置构造：公开关联函数，供组合和测试使用
    config(&self) -> &MargatroidConfig
        读取配置：公开方法，供组合根在安装Plugin前取得Server监听地址
    impl Plugin for ConfigPlugin
        Plugin：安装MargatroidConfig，重复安装时panic
        build(self, app: &mut App)
            构建插件：公开trait方法，插入验证后的配置并拒绝重复安装或不存在的Schedule

ConfigError：配置错误，公开枚举--不回显配置正文
    ReadFailed(PathBuf)--配置文件无法读取
    TooLarge--配置正文超过64 KiB
    DecodeFailed--TOML格式或字段不符合ConfigDocument
    InvalidServerBind--server.bind不是SocketAddr
    EmptyTargets(&'static str)--指定出站类别没有目标
    InvalidTarget(&'static str)--指定出站类别包含非法目标
    DuplicateTarget(&'static str)--指定出站类别包含重复目标
    impl fmt::Display for ConfigError
        Display：公开trait实现，输出不含配置正文的稳定描述
    impl std::error::Error for ConfigError
        Error：公开trait实现
```

私有：
```text
ConfigDocument：全局配置文档，私有结构体--拒绝未知字段
    server: ServerDocument--Server启动配置
    outbound: OutboundDocument--四类WebSocket出站目标
    impl TryFrom<ConfigDocument> for MargatroidConfig
        Error = ConfigError
        try_from(document: ConfigDocument) -> Result<MargatroidConfig, ConfigError>
            转换配置：逐类解析目标并调用MargatroidConfig::new完成统一验证

ServerDocument：Server配置文档，私有结构体--拒绝未知字段
    bind: String--必须可解析为SocketAddr

OutboundDocument：出站配置文档，私有结构体--拒绝未知字段
    logs: Vec<String>
    backend_state: Vec<String>
    member_messages: Vec<String>
    streaming_member_messages: Vec<String>
```

## 函数

私有：
```text
decode_targets(field: &'static str, targets: Vec<String>) -> Result<Vec<WebSocketMessageTarget>, ConfigError>
    解析目标：接受broadcast、type:<值>和name:<值>，其余返回InvalidTarget(field)

validate_targets(field: &'static str, targets: &[WebSocketMessageTarget]) -> Result<(), ConfigError>
    验证目标：要求集合非空、Type和Name值合法且同一类别内目标不重复

valid_value(value: &str) -> bool
    验证目标值：要求非空且不包含控制字符
```

## 逻辑

```text
daemon
    -> ConfigPlugin::open(<data_root>/config.toml)
    -> 读取server_bind构造ServerPlugin
    -> 在DtoPlugin和InferencePlugin之前安装
    -> MargatroidConfig Resource
        -> DtoPlugin读取logs、backend_state和member_messages
        -> InferencePlugin读取streaming_member_messages
```

## 边界

```text
ConfigPlugin负责：读取、解析、验证并安装Server和出站全局配置
ConfigPlugin不负责：查询WebSocket连接、序列化消息或发送消息
ServerPlugin负责：将WebSocketMessageTarget解析为具体连接发送器的基础能力
config.toml格式：监听地址位于[server]表，四组目标位于[outbound]表；目标字符串只允许broadcast、type:<连接类型>和name:<连接名称>
```
