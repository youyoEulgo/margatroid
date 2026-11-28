# agent_driver_core

`agent_driver_core` 提供 Lua 程序、初始 TOML 与 Driver UI 的文件读取能力。
公开 API 返回 owned 材料或共享观察状态，供资源加载与网络适配接入。

## 核心对象

- `AgentDriverInput { program, origin }` 是读取后的程序与来源路径。
- `AgentDriverReadLimits` 默认程序预算 4 MiB、配置预算 256 KiB。
- `DriverUiBundle` 保存 revision、entry 与文件数组；每个文件有 path、media_type 和 data。
- `DriverUiState` 表达 Initializing、Ready 或 Failed，Ready 携带可选 bundle。
- `DriverUiWatch` 与 `DriverUiMonitor` 分别提供状态句柄和实际监控执行体。

## 读取程序、配置与 UI

示例函数可放入依赖 `agent_driver_core` 的包，路径由资源解析层提供：

```rust
use agent_driver_core::{
    AgentDriverReadLimits, DriverUiReadLimits,
    read_driver, read_driver_ui, read_initial_settings,
};
use std::path::Path;

fn load(program: &Path, settings: &Path, ui: &Path)
    -> Result<(), Box<dyn std::error::Error>>
{
    let limits = AgentDriverReadLimits::default();
    let input = read_driver(program, &limits)?;
    let initial = read_initial_settings(settings, limits.settings_bytes)?;
    let bundle = read_driver_ui(ui, DriverUiReadLimits::default())?;
    assert_eq!(input.origin, program);
    let _materials = (input.program, initial, bundle);
    Ok(())
}
```

read_driver 读取给定文件为 UTF-8 文本；read_initial_settings 额外校验 TOML Table。
调用方将程序交给 LuaRuntime、初始配置交给存储入口。
文件过大、路径错误、UTF-8 错误与 TOML 错误通过 AgentDriverError 返回。

UI 默认 raw_bytes 预算为 32 MiB，计入相对路径、MIME 类型和文件数据。
两次扫描得到同一集合后返回稳定 bundle；读取期间变化返回 Unstable。
UI 目录缺失返回 `Ok(None)`，存在目录缺少 index.html 返回 EntryMissing。
规范路径约束在 UI 根目录内，非法路径返回 PathInvalid。

## 持续观察

`open_driver_ui_watch(path, callback)` 返回 `(watch, monitor)`。
在 Tokio 中运行 `run_driver_ui_monitor(monitor)`，通过 `watch.state()` 读取当前状态。
notify 变化经过 100ms 安静窗口后重新扫描，实际状态变化调用 callback。
停止时调用 `watch.cancel()` 并等待监控 Future 完成，文件 worker 随之收口。
完整可运行调用与取消验证见 [UI 测试](tests/ui.rs)。

## 查阅与验证

- [公开入口与读取](src/lib.rs)、[预算类型](src/types.rs)、[UI 实现](src/ui.rs)。
- [程序读取测试](tests/read_driver.rs)：精确预算、初始配置与错误。
- [UI 测试](tests/ui.rs)：完整资源、revision、观察变化和退出等待。
- [领域导航](../README.md) 与 [设计](../DESIGN.md)。

运行 `cargo test -p agent_driver_core` 验证本包。
