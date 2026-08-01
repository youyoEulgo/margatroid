# config_plugin

`config_plugin` 提供 V3 配置加载边界。

## 职责

- 注册 `ConfigStore` Resource。
- 从 TOML 文件加载应用配置。
- 发出配置加载、重新加载和失败事件。

## 公开事件

- `ConfigLoadRequested`
- `ConfigLoaded`
- `ConfigReloaded`
- `ConfigLoadFailed`

## 公开 Resource

- `ConfigStore`

## Stage 注册

- 可选的自动加载在 `Stage::Startup` 执行。
- 显式加载请求在 `Stage::Update` 消费。

## 最小示例

```rust
use config_plugin::{ConfigLoadRequested, ConfigPlugin};
use core_plugin::App;

let mut app = App::new();
app.add_plugins(ConfigPlugin::new());
app.world().send_event(ConfigLoadRequested::new("config.toml"));
app.tick();
```

## 边界

该 Plugin 不创建 Workspace、不构造 LLM Provider，也不启动 HTTP Server。
