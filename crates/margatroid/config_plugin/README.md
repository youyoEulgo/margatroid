# ConfigPlugin

`ConfigPlugin` 读取 Margatroid 主目录的 `config.toml`，校验Server监听地址、入口策略以及日志、后端状态、
完整成员消息和流式成员消息四类 WebSocket 目标，并把不可变 `MargatroidConfig` 插入 World。组合根使用
监听地址与入口策略构造ServerPlugin；DtoPlugin和InferencePlugin只读取该Resource，不自行读取配置文件。

```toml
[server]
bind = "127.0.0.1:3939"
allow = "localhost"

[outbound]
logs = ["type:cli", "type:webui"]
backend_state = ["type:webui"]
member_messages = ["type:webui"]
streaming_member_messages = ["type:webui"]
```

`server.allow` 是入口策略，缺省为 `"localhost"`：只接受回环对端，并且带 `Origin` 的请求要求其
authority 与 `Host` 一致。也可以写 `"all"`（需同时放宽 `bind`）或 IP/CIDR 数组。策略由
`IngressPolicy` 实现 ServerPlugin 的 `HandshakeGuard` 接缝，因此业务词表留在本 crate，
基础设施只接收判定结果。`allow` 放宽而 `bind` 仍为回环、数组里出现 `0.0.0.0/0` 或裸 `0.0.0.0`、
未知标量都会在加载期失败，不会静默放宽。

`server.bind` 必须是完整的Socket地址。目标支持 `broadcast`、`type:<连接类型>` 和
`name:<连接名称>`。四个字段都必须至少包含一个目标，同一字段不能重复目标，未知字段和未知目标
前缀会导致配置加载失败。
