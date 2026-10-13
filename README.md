# Margatroid

用 Rust 实现的多智能体协作运行时。

项目仍在早期开发中，功能并不完善，API 和架构可能随时变动。

当前 workspace 使用统一资源集合、共享组件和事件，以及 Agent 自有 MCL FIFO。CLI 负责编译
Workspace 文件、通过 WebSocket 发送启动和停止请求并打印日志；daemon 安装插件并提供 Web UI。
每个 Workspace 使用专属本地异步执行线程运行 Driver。
资源使用显式 `IMPORT MESSAGE / TOOL / SANDBOX` 建立 alias，GET、INJECT 左侧和 effect 括号共用同类型数组求值。
Prompt 按真实 role 转换为消息，工具调用直接接受一条 assistant 消息及可见 TOOL 数组，不再设置 RESOURCE 或 TOOL_CALL 字段。
Web UI 使用显式 MCL 工作台，消息注入需要配置 Driver 的字段路径，不猜测上下文、不自动触发推理。
旧 RefBlock、独立资源身份组件、邮箱、实时 STATE 和可见性绑定接口已移除，这是一次破坏兼容性的迁移。

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

## 重写后的约定

- 资源包通过五类加载插件挂到目标实体的 `ResourceSet<T>`；重复 IMPORT 重载，失败保留旧内容。
- 每条 MCL 除 `history_append` 外先记录原指令；执行后的历史失败单独返回 `history_error`，不会重做工具。
- 工具 runner 是独立程序，构建 daemon 时应同时构建 `cargo build -p tool_plugin --bin tool_runner`，或设置 `MARGATROID_TOOL_RUNNER` 指向它。
- Linux shell 支持显式 PTY；有激活沙箱但后端缺失时拒绝执行，不降级成无沙箱。
- 首版不支持长期任务转交。真实清理失败保留活动及宿主并返回错误，不自动重启或删除。
- 工作区必须先停止 Agent、等待执行作用域和 MCL 回执收口，再关闭线程；直接丢弃 App 不是强制清理协议。
- 历史投影目前没有真实时间戳，Web UI 显示不可用；不将 `created_at_ms = 0` 当作真实执行时间。

## 验证

```text
cargo test --workspace --all-targets
cargo test --workspace --doc
cd ui
npm test
npm run build
```
