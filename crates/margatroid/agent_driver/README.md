# AgentDriver

AgentDriver 领域负责读取 Lua 程序材料、校验初始配置，以及读取和观察 Driver UI 文件。
实际 Lua 执行由 LuaRuntime 承担，Agent 生命周期与持久状态分别接入各自领域。
本目录提供一个 Rust 包 [agent_driver_core](core/README.md)。

## 核心概念

- `AgentDriverInput` 包含 Lua 程序文本与调用方提供的来源路径。
- `AgentDriverReadLimits` 指定程序和初始配置的字节预算。
- `DriverUiBundle` 是带内容 revision 的完整 UI 静态文件集合。
- `DriverUiWatch` 提供共享状态读取与取消，`DriverUiMonitor` 承担实际文件观察。

## 一次材料加载

调用方先解析程序资源地址，取得实际 Lua 文件路径，再调用 read_driver。
需要初始化配置时单独调用 read_initial_settings 校验 TOML 文本，
随后由存储领域把初值写入 Agent 持久配置。
程序与配置的路径分别由调用方指定。

UI 使用调用方提供的目录路径：目录中以 index.html 为入口，包含编译后的脚本、
样式和其他静态文件。read_driver_ui 返回 owned 文件集合，文件包含相对路径、
MIME 类型与字节数据；内容 revision 由路径、类型和数据共同计算。
目录缺失时返回 `Ok(None)`，存在目录且入口缺失时返回 EntryMissing。

持续观察使用 open_driver_ui_watch 创建句柄与 monitor，
交给 Tokio 运行 run_driver_ui_monitor。观察在稳定初值、内容变化、错误与恢复时
发布 DriverUiState，装配层把该状态转换为客户端交付。
来源结束时取消 watch 并等待 monitor 返回。

## 开发导航

| 位置 | 用途 |
| --- | --- |
| [core 接入](core/README.md) | 程序/TOML/UI 的真实 Rust 调用与预算 |
| [读取实现](core/src/lib.rs) | 字节预算、UTF-8 和 TOML 校验 |
| [UI 实现](core/src/ui.rs) | 静态文件扫描、revision 和监控 |
| [LuaRuntime](../lua_runtime/README.md) | 执行程序与宿主注入能力 |
| [Storage](../storage/README.md) | Agent 初始配置和持久会话 |
| [设计](DESIGN.md) | 文件类型与观察调用关系 |

程序读取使用同步文件 API，异步宿主把读取安排到 blocking worker。
UI monitor 已将扫描安排到 Tokio blocking worker，并在结束时等待扫描收口。

运行 `cargo test -p agent_driver_core` 验证材料读取、UI 状态与监控生命周期。
