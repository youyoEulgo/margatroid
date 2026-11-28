# Config

Config 领域提供 daemon 启动时使用的服务器入口、日志和工具容量配置。
文件入口读取 `config.toml`，校验后形成 `MargatroidConfig`，
由应用装配为 World Resource，供服务器和工具执行设施取值。

## 核心概念

- `MargatroidConfig` 是校验后的启动配置快照。
- `IngressPolicy` 表达回环、全部地址或 IP/CIDR 列表策略，并检查 Origin 与 Host。
- `LogLevel` 与可选 filter 提供日志装配参数，filter 指定时作为完整过滤表达式。
- `FileToolConfig` 保留配置文件中的可选容量，compose 补齐工具领域默认值。
- `ConfigPlugin` 负责文件读取和 Resource 安装。

## 配置一个本地实例

```toml
[server]
bind = "127.0.0.1:3939"
allow = "localhost"

[log]
level = "info"

[tools]
lua_vm_memory_limit = "2G"
task_output_buffer_window = "128M"
```

`server.bind` 为 IP 与端口组成的 SocketAddr。allow 默认 localhost，
也支持 all 和 IP/CIDR 字符串数组；回环 bind 配合回环入口策略。
请求带 Origin 时按 HTTP(S) 协议、主机名与有效端口检查同源。

日志级别默认 info，支持 off、error、warn、info、debug、trace。
filter 采用含有效字符的字符串，tracing 表达式语法由应用日志装配校验。
工具容量使用正整数容量字符串，K/M/G/T 按 1024 进制，可附 B 或 iB。
工具默认值由工具领域提供：Lua VM 2 GiB，任务输出窗口 128 MiB。
前者约束工具 VM 分配器，后者控制宿主保存的任务输出窗口。

应用启动时调用 ConfigPlugin::open，再安装快照；修改配置后重启 daemon 加载新值。
具体调用见 [config_plugin](plugin/README.md)，程序构造见 [config_core](core/README.md)。

## 开发导航

- [文件读取与安装](plugin/README.md)：TOML、错误和启动调用。
- [配置值与入口判断](core/README.md)：程序化构造和 IngressRequest。
- [compose](../compose/README.md)：把文件容量转换为工具执行限额。
- [设计](DESIGN.md)：配置对象和职责关系。

运行 `cargo test -p config_core -p config_plugin` 验证配置值与文件解析。
