# ConfigPlugin

`ConfigPlugin` 读取 Margatroid 主目录的 `config.toml`，校验日志、后端状态、完整成员消息和流式成员
消息四类 WebSocket 目标，并把不可变 `MargatroidConfig` 插入 World。DtoPlugin 和 InferencePlugin
只读取该 Resource，不自行读取配置文件。

```toml
[outbound]
logs = ["type:cli", "type:webui"]
backend_state = ["type:webui"]
member_messages = ["type:webui"]
streaming_member_messages = ["type:webui"]
```

目标支持 `broadcast`、`type:<连接类型>` 和 `name:<连接名称>`。四个字段都必须至少包含一个目标，
同一字段不能重复目标，未知字段和未知目标前缀会导致配置加载失败。
