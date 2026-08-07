# Margatroid

用 Rust 实现的多智能体协作运行时。

项目仍在早期开发中，功能并不完善，API 和架构可能随时变动。

当前 workspace 保留 mecs 基础设施和正在重构的 Margatroid V3 业务 crate。CLI 负责编译
Workspace 文件、通过 WebSocket 发送 Workspace 启动请求并打印后端日志；daemon 负责安装插件、
创建 Workspace 和提供 WebSocket 后端。LLM 消息输入输出仍未接入 CLI。

启动后端：

```text
cargo run -p margatroid_daemon -- --data-root ~/.margatroid
```

daemon 默认监听 `127.0.0.1:3939`，并要求 `~/.margatroid/models.toml` 是有效的模型路由配置。
