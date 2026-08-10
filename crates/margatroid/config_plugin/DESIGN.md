# ConfigPlugin

## 类型

公开：
```text
WebSocketMessageTarget：WebSocket消息目标，公开枚举--保存尚未解析为连接发送器的目标
    Broadcast--全部当前连接
    Type(String)--指定连接类型
    Name(String)--指定连接名称

MargatroidConfig：Margatroid全局配置，公开只读Resource--从主目录config.toml完整加载
    logs: Vec<WebSocketMessageTarget>--日志、Workspace启停结果及成员失败异常的目标
    backend_state: Vec<WebSocketMessageTarget>--完整后端状态目标
    member_messages: Vec<WebSocketMessageTarget>--完整成员消息目标
    streaming_member_messages: Vec<WebSocketMessageTarget>--流式成员消息目标
    new(logs, backend_state, member_messages, streaming_member_messages) -> Result<Self, ConfigError>
        构造配置：公开关联函数，验证四组目标非空、合法且不重复
    logs(&self) -> &[WebSocketMessageTarget]
    backend_state(&self) -> &[WebSocketMessageTarget]
    member_messages(&self) -> &[WebSocketMessageTarget]
    streaming_member_messages(&self) -> &[WebSocketMessageTarget]
    impl Resource for MargatroidConfig

ConfigPlugin：全局配置插件，公开结构体--读取配置并安装只读Resource
    config: MargatroidConfig--已完整验证的配置
    open(path: impl Into<PathBuf>) -> Result<Self, ConfigError>
        打开配置：公开关联函数，有界读取并解析config.toml
    new(config: MargatroidConfig) -> Self
        使用配置构造：公开关联函数，供组合和测试使用
    impl Plugin for ConfigPlugin
        Plugin：安装MargatroidConfig，重复安装时panic

ConfigError：配置错误，公开枚举--不回显配置正文
```

## 逻辑

```text
daemon
    -> ConfigPlugin::open(<data_root>/config.toml)
    -> 在DtoPlugin和InferencePlugin之前安装
    -> MargatroidConfig Resource
        -> DtoPlugin读取logs、backend_state和member_messages
        -> InferencePlugin读取streaming_member_messages
```

## 边界

```text
ConfigPlugin负责：读取、解析、验证并安装全局配置
ConfigPlugin不负责：查询WebSocket连接、序列化消息或发送消息
ServerPlugin负责：将WebSocketMessageTarget解析为具体连接发送器的基础能力
config.toml格式：四组目标位于[outbound]表，目标字符串只允许broadcast、type:<连接类型>和name:<连接名称>
```
