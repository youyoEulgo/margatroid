# ConfigPlugin

`ConfigPlugin` 读取 Margatroid 主目录的 `config.toml`，校验Server监听地址以及日志、后端状态、完整
成员消息和流式成员消息四类 WebSocket 目标，并把不可变 `MargatroidConfig` 插入 World。组合根使用
监听地址构造ServerPlugin；DtoPlugin和InferencePlugin只读取该Resource，不自行读取配置文件。

```toml
[server]
bind = "127.0.0.1:3939"

[outbound]
logs = ["type:cli", "type:webui"]
backend_state = ["type:webui"]
member_messages = ["type:webui"]
streaming_member_messages = ["type:webui"]
```

`server.bind` 必须是完整的Socket地址。目标支持 `broadcast`、`type:<连接类型>` 和
`name:<连接名称>`。四个字段都必须至少包含一个目标，同一字段不能重复目标，未知字段和未知目标
前缀会导致配置加载失败。
