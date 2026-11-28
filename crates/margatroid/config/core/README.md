# config_core

`config_core` 定义经过校验的启动配置值，供文件读取和应用装配共同使用。
它依赖 mecs core 与 serde，`MargatroidConfig` 可作为 World Resource 安装。

## 核心对象

- `MargatroidConfig::new` 接收服务器地址、日志级别和可选过滤表达式。
- `with_ingress` 校验并附加来源策略，`with_tools` 校验可选工具容量。
- `IngressPolicy` 提供 peer、Origin 与 Host 的组合判断。
- `CidrBlock` 解析 IPv4/IPv6 主机或 CIDR，并提供地址包含判断。
- `FileToolConfig` 的两个可选 usize 字段表示文件给出的容量字节数。

## 构造配置并检查请求

示例可放入依赖 `config_core` 的包。网络适配把实际 peer、Origin 与 Host
组成 IngressRequest，再调用策略的 evaluate 方法。

```rust
use config_core::{IngressRequest, LogLevel, MargatroidConfig};

fn main() -> Result<(), config_core::ConfigError> {
    let config = MargatroidConfig::new(
        "127.0.0.1:3939".parse().unwrap(),
        LogLevel::Info,
        Some("info,application=debug".into()),
    )?;
    let request = IngressRequest {
        peer: "127.0.0.1:12345".parse().unwrap(),
        origin: Some("http://localhost:3939".into()),
        host: Some("localhost:3939".into()),
    };
    assert!(config.server_ingress().evaluate(&request).is_ok());
    assert_eq!(config.log_filter(), Some("info,application=debug"));
    Ok(())
}
```

默认入口策略为 Localhost。Any 或 Addresses 可以通过 with_ingress 指定，
回环 bind 只接受回环范围的策略。跨地址范围的服务可使用通配 bind 配合显式策略。
filter 为空白字符串时返回 InvalidLogFilter；容量为零时返回含字段路径的 InvalidToolLimit。
入口判断失败时 evaluate 返回具体原因，网络边界据此生成响应。

## 容量与装配

`FileToolConfig` 字段使用 Option 表达用户选择，应用装配将其与工具默认限额组合。
serde 读取容量字符串，如 `2G`、`128MiB`、`4096`，转换成正整数字节数；
序列化选择可精确表示该值的最大单位。

## 查阅与验证

- [公开入口](src/lib.rs)：构造、getter 与 Resource 实现。
- [配置类型](src/types.rs)：CIDR、入口判断和容量编解码。
- [配置测试](tests/config.rs)：程序化配置和请求策略。
- [文件接入](../plugin/README.md) 与 [设计](../DESIGN.md)。

运行 `cargo test -p config_core` 验证核心配置行为。
