# config_plugin

`config_plugin` 读取启动 TOML，校验后把 MargatroidConfig 安装到 World。
文件读取发生在 `open`；`build` 发布已经加载的配置快照。

## 核心入口

- `ConfigPlugin::open(path)` 从 UTF-8 TOML 加载配置。
- `ConfigPlugin::new(config)` 接收调用方构造的已校验配置。
- `config()` 在安装前提供配置引用，便于创建服务器和日志设施。
- 安装后通过 World 的 `get_resource::<MargatroidConfig>()` 读取快照。

## 启动时加载

先按 [领域配置示例](../README.md) 准备配置文件，再调用：

```rust
use config_plugin::{ConfigError, ConfigPlugin, MargatroidConfig};
use core_plugin::App;
use std::path::Path;

fn load(path: &Path) -> Result<App, ConfigError> {
    let plugin = ConfigPlugin::open(path)?;
    let bind = plugin.config().server_bind();
    let mut app = App::new();
    app.add_plugin(plugin);
    let installed = app.world().get_resource::<MargatroidConfig>().unwrap();
    assert_eq!(installed.server_bind(), bind);
    Ok(app)
}
```

最小文件包含 `[server]` 与 `bind = "127.0.0.1:3939"`。
log 与 tools 可省略，allow 默认为 localhost，日志级别默认为 info。
应用装配取得这些值后创建实际日志、网络和工具执行设施。
每个 App 安装一次 ConfigPlugin。

## 文件校验

读取预算为 64 KiB。读取失败返回携带路径的 ReadFailed，超预算返回 TooLarge，
UTF-8 或 TOML 解码失败返回 DecodeFailed。
配置结构包含 server、log、tools，额外字段由严格反序列化返回 DecodeFailed。
SocketAddr、日志级别、入口策略和容量经过各自的值校验。

allow 接受 localhost、all 或 IP/CIDR 数组；字符串形式同时接受对应的
127.0.0.1 与 0.0.0.0。容量按正整数容量字符串读取，格式和示例见
[config_core](../core/README.md)。

## 查阅与验证

- [插件入口](src/lib.rs)：open/new、快照访问和 Resource 安装。
- [文件解析](src/handler.rs)：预算、TOML 结构和值转换。
- [配置插件测试](tests/config_plugin.rs)：最小文件、完整配置、错误和快照安装。
- [领域设计](../DESIGN.md)：配置读取与使用关系。

执行 `cargo test -p config_plugin` 验证文件入口。
