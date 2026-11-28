# margatroid 应用入口

`margatroid` 包把 Compose 装配结果作为服务进程运行，并提供安装器入口。
维护应用启动、信号处理和安装行为时，从本包进入；领域装配见
[compose](../../crates/margatroid/compose/README.md)，使用与安装步骤见[项目 README](../../README.md)。

## 两个可执行入口

[Cargo.toml](Cargo.toml) 声明两个 binary：

- `margatroid`：定位数据目录、创建资源目录、调用 `build_app`，持续推进应用运行与关停。
- `margatroid-install`：接收 checkout 路径，安装构建产物、配套资源与初始配置。

服务入口从 Unix 的 `HOME` 或 Windows 的 `USERPROFILE` 取主目录，使用其中的
`.margatroid`，并将当前可执行文件路径交给 `ApplicationOptions.runner_path`。
启动时建立 `workspaces`、`agent-drivers`、`prompts`、`sandboxes` 和 `tools` 目录；
配置由 Compose 装配的领域读取。主目录变量缺失、配置读取失败或装配失败时返回错误。
[config.example.toml](config.example.toml) 展示监听地址、来源和日志等级配置。

## 一次服务运行

从 workspace 根目录构建并启动：

```sh
cargo build --release -p margatroid
./target/release/margatroid
```

准备好的数据目录包含 `config.toml`、`models.toml` 以及所需资源定义。
`run` 创建 App、取得 `AxumHandle` 与 `RuntimeHandle`，随后进入 `run_until`。
Axum 成功绑定后，启动提示通过 tracing 输出实际监听地址、Web UI 和 `/ws` 地址。
监听端口为 `0` 时提示使用实际分配端口；通配监听地址的浏览器链接使用相应回环地址。

Unix 的 SIGINT、SIGTERM，以及其他平台的 Ctrl+C，设置停止标记并唤醒运行循环。
退出循环后，入口反复等待 `compose::shutdown_app` 完成领域清理；清理期间继续监听信号。
领域清理错误进入诊断与继续监督流程，Axum 最终错误在领域收口后作为进程错误返回。
完整时序见 [main.rs](src/main.rs) 与[关停测试](tests/shutdown.rs)。

## 工具 runner 模式

首个参数为 `--margatroid-tool-supervisor` 或 `--margatroid-tool-worker` 时，
入口创建单线程 Tokio runtime 并交给 `tool_plugin::runner_main`，以其返回值作为退出码。
Linux supervisor 接受 `--margatroid-tool-supervisor --sandbox <backend> <policy>`，
worker 使用 `--margatroid-tool-worker`；参数解析、JSON 协议和子进程监督见
[tool handler](../../crates/margatroid/tool/plugin/src/handler.rs)。
服务通过同一可执行文件承载这些模式，独立工具入口另见
[tool_runner.rs](../../crates/margatroid/tool/plugin/src/bin/tool_runner.rs)。

## 安装器职责

调用形态为 `margatroid-install <checkout> [--skip-landstrip] [--update-resources]`。
安装器检查 checkout 内的 release 产物 `margatroid`、`tool_runner`、`shell-client`、
`mcp-client`，以及主 UI、assistant UI 与 subagent UI 的预编译入口。
默认可执行文件目录为主目录下的 `.local/bin`，`MARGATROID_BIN_DIR` 指定另一安装目录。

默认流程下载并校验 Landstrip 0.22.5，执行版本与 doctor 检查；
`--skip-landstrip` 选择直接安装本地构建产物。资源复制保留目标中已有文件，
`--update-resources` 选择覆盖资源。两种模式都刷新 bash、bash-exec、mcp-register 的 helper。
`config.toml` 与 `models.toml` 在目标首次创建时写入，已有配置继续保留。
文件先写入同目录 staging 文件，完成同步后 rename 到目标；路径检查遇到符号链接返回错误。
安装与归档校验的实现及单元测试见 [install.rs](src/install.rs)。

## 源码与测试导航

- [main.rs](src/main.rs)：服务启动、runner 分派、监听地址提示与信号处理。
- [lifecycle.rs](tests/lifecycle.rs)：初始目录创建与显式配置要求。
- [shutdown.rs](tests/shutdown.rs)：真实进程、失败启动、重复信号和后代回收。
- [bundled_drivers.rs](tests/bundled_drivers.rs)：随包 Driver、资源及模型文件 API。
- [mcp_update.rs](tests/mcp_update.rs)：MCP 注册更新与初始化指令保存。
- [web_tools.rs](tests/web_tools.rs)：随包 Web 工具请求与结果处理。
