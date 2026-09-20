# Margatroid

用 Rust 实现的多智能体协作运行时。

项目仍在早期开发中，功能并不完善，API 和架构可能随时变动。

当前 workspace 保留 mecs 基础设施和正在重构的 Margatroid V3 业务 crate。CLI 负责编译
Workspace 文件、通过 WebSocket 发送 Workspace 启动请求并打印后端日志；daemon 负责安装插件、
创建 Workspace、提供 WebSocket 后端，并直接提供 Web 界面。LLM 消息输入输出仍未接入 CLI。

## 目录

```text
apps/       margatroid 可执行程序（cli、daemon）
crates/mecs       基础设施
crates/margatroid 业务插件
ui/               Web 界面源码（Vue + Vite）
```

## 构建与启动

daemon 在编译期把 `ui/dist` 嵌入二进制，所以首次构建前需要先构建界面：

```text
./scripts/build-ui.sh
cargo run -p margatroid_daemon
```

`ui/dist` 不入库；改了界面就要重新跑一次 `scripts/build-ui.sh`，再重新编译 daemon。
只改后端时可以跳过界面构建，用上一次的 `ui/dist`。

## 访问

daemon 同时提供界面与 WebSocket：

```text
http://127.0.0.1:3939/      Web 界面
ws://127.0.0.1:3939/ws      WebSocket 后端
```

界面默认连到它自己的来源（同源），所以从上面这个地址打开即可，不需要填后端地址；
界面上仍可手动指定其它后端。

daemon不接受启动参数，固定读取 `~/.margatroid/config.toml` 和 `~/.margatroid/models.toml`。
监听地址由 `config.toml` 的 `server.bind` 配置，入口策略由同节的 `server.allow` 配置（默认 `"localhost"`，即只接受回环对端且要求 `Origin` 与 `Host` 同源）。
