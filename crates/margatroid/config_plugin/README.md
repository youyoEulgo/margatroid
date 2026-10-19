# ConfigPlugin

`ConfigPlugin` 读取 Margatroid 主目录的 `config.toml`，校验Server监听地址、入口策略以及日志、后端状态
两类 WebSocket 目标，并把不可变 `MargatroidConfig` 插入 World。组合根使用
监听地址与入口策略构造ServerPlugin；DtoPlugin只读取该Resource，不自行读取配置文件。

```toml
[server]
bind = "127.0.0.1:3939"
allow = "localhost"

[outbound]
logs = ["type:cli", "type:webui"]
backend_state = ["type:webui"]
```

模型路由只在 `~/.margatroid/models.toml` 配置，加载/重载、provider/api_type组合、凭据与API前缀校验归InferencePlugin。本crate不复制路由配置或依赖其执行实现；本次没有检查或修改主目录私有配置，不宣称已有路由已迁移。

可选日志配置为：

```toml
[log]
level = "info"
# filter = "warn,margatroid=debug"
```

`level` 接受 `off/error/warn/info/debug/trace`，缺省为 `info`；`filter` 若提供须非空，语法由log_plugin的EnvFilter在装配时校验。filter完整替代level指令，例如level为debug而filter为warn时按warn过滤，不叠加debug。日志流仍受实际tracing过滤器控制，出站目标只决定已产生记录的去向。

`[outbound]` 仅接受 `logs` 与 `backend_state`；旧 `member_messages`、`streaming_member_messages` 已删除，会作为未知字段拒绝。推理增量与命令关联回执由运行时身份和请求上下文路由，不新增出站配置类别。

本文只读核对源码，未运行构建或测试；DTO Handler 重置后的整树恢复未验证，不能据此认定catalog或生命周期端到端已恢复。

`server.allow` 是入口策略，缺省为 `"localhost"`：只接受回环对端，并且带 `Origin` 的请求要求其
authority 与 `Host` 一致。也可以写 `"all"`（需同时放宽 `bind`）或 IP/CIDR 数组。策略由
`IngressPolicy` 实现 ServerPlugin 的 `HandshakeGuard` 接缝，因此业务词表留在本 crate，
基础设施只接收判定结果。`allow` 放宽而 `bind` 仍为回环、数组里出现 `0.0.0.0/0` 或裸 `0.0.0.0`、
未知标量都会在加载期失败，不会静默放宽。

`server.bind` 必须是完整的Socket地址。目标支持 `broadcast`、`type:<连接类型>` 和
`name:<连接名称>`。两个字段都必须至少包含一个目标，同一字段不能重复目标，未知字段和未知目标
前缀会导致配置加载失败。
