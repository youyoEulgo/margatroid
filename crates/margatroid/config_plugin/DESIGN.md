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

lib 只放 Plugin 与安装逻辑。MargatroidConfig 是配置 Resource，放在 types。

## 类型

公开：
```text
ConfigPlugin：全局配置插件，公开结构体--读取并验证config.toml后安装MargatroidConfig
    config: MargatroidConfig--已完整验证的配置，私有
    open(path: impl Into<PathBuf>) -> Result<Self, ConfigError>
        打开配置：公开关联函数，有界读取并解析config.toml
    new(config: MargatroidConfig) -> Self
        使用配置构造：公开关联函数，供组合和测试使用
    config(&self) -> &MargatroidConfig
        读取配置：公开方法，供组合根在安装Plugin前取得Server监听地址
    impl Plugin for ConfigPlugin
        Plugin：公开trait实现
        build(self, app: &mut App)
            安装插件：插入MargatroidConfig Resource，重复安装时panic
```

## 逻辑

```text
daemon
    -> ConfigPlugin::open(<data_root>/config.toml)
    -> 读取server_bind构造ServerPlugin
    -> 安装MargatroidConfig Resource
        -> DtoPlugin读取logs、backend_state和member_messages
        -> InferencePlugin读取streaming_member_messages
```

# system

```text
ConfigPlugin 不定义 System。
```

# handler

```text
ConfigPlugin 不定义 handler 函数；配置解析和验证逻辑属于 types 中的 MargatroidConfig 与 ConfigDocument。
```

# events

```text
ConfigPlugin 不定义 ECS 事件。
```

# types

types 放配置 Resource、目标类型、配置文档和解析验证函数。

## 常量

crate公开：
```text
MAX_CONFIG_BYTES: usize = 64 * 1024--配置文件最大字节数
```

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
    new(server_bind: SocketAddr, logs: Vec<WebSocketMessageTarget>, backend_state: Vec<WebSocketMessageTarget>, member_messages: Vec<WebSocketMessageTarget>, streaming_member_messages: Vec<WebSocketMessageTarget>) -> Result<Self, ConfigError>
        构造配置：公开关联函数，保存监听地址并验证四组目标非空、合法且不重复
    server_bind(&self) -> SocketAddr
        读取监听地址：公开方法
    logs(&self) -> &[WebSocketMessageTarget]
        读取日志目标：公开方法
    backend_state(&self) -> &[WebSocketMessageTarget]
        读取后端状态目标：公开方法
    member_messages(&self) -> &[WebSocketMessageTarget]
        读取成员消息目标：公开方法
    streaming_member_messages(&self) -> &[WebSocketMessageTarget]
        读取流式成员消息目标：公开方法
    impl Resource for MargatroidConfig
```

crate公开：
```text
ConfigDocument：全局配置文档，crate公开结构体--拒绝未知字段
    server: ServerDocument--Server启动配置
    outbound: OutboundDocument--四类WebSocket出站目标
    impl TryFrom<ConfigDocument> for MargatroidConfig
        Error = ConfigError
        try_from(document: ConfigDocument) -> Result<MargatroidConfig, ConfigError>
            转换配置：解析server.bind并逐类解析目标，调用MargatroidConfig::new完成统一验证

ServerDocument：Server配置文档，crate公开结构体--拒绝未知字段
    bind: String--必须可解析为SocketAddr

OutboundDocument：出站配置文档，crate公开结构体--拒绝未知字段
    logs: Vec<String>--日志目标
    backend_state: Vec<String>--后端状态目标
    member_messages: Vec<String>--成员消息目标
    streaming_member_messages: Vec<String>--流式成员消息目标
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

# error

error 放 Error 类型和公开错误分类。

## 类型

公开：
```text
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

## 逻辑

```text
ConfigError 的 &'static str 字段只用于标识配置字段名（logs、backend_state、member_messages、streaming_member_messages），不回显目标值或配置正文。
```

## 边界

```text
ConfigPlugin负责：读取、解析、验证并安装Server和出站全局配置。
ConfigPlugin不负责：查询WebSocket连接、序列化消息或发送消息。
ServerPlugin负责：将WebSocketMessageTarget解析为具体连接发送器的基础能力。
config.toml格式：监听地址位于[server]表，四组目标位于[outbound]表；目标字符串只允许broadcast、type:<连接类型>和name:<连接名称>。
```
