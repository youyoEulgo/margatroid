# lua_runtime_core：Lua 环境与宿主协议

`lua_runtime_core` 提供 Lua 程序、值桥接、共享宿主对象与独立 VM 的启停协议。
它通过 mecs 组件与事件表达生命周期，通过通用回调表达业务能力。
[依赖声明](Cargo.toml) 与 [公开导出](src/lib.rs) 给出 crate 的 Rust 接入面。

## 核心对象

| 类型 | 职责与来源 |
| --- | --- |
| `LuaProgram` | 调用方传入的源码及 origin 路径 |
| `LuaEnvironment` | 调用方注入的 globals 与 modules |
| `LuaValue` | 显式区分数组、对象、null 和标量的共享值 |
| `HostDefinition` | 安装宿主时固定 kind、context 和 arguments |
| `Margatroid` | 保存可信宿主定义与执行阶段回调 |
| `LuaExecutionIdentity` | 保存 source、workspace、address、instance 与取消 token |
| `LuaServices` | 接收上述身份、方法名和 JSON 参数的异步业务分派器 |

环境和共享值见 [types](src/types.rs)，身份与回调见 [services](src/services.rs)，宿主定义见 [host](src/host.rs)。

## 构建一次宿主环境

宿主先创建带 VM 内存预算的 Lua 状态，准备配置阶段 globals，加载程序并调用 configure。
配置确认后恢复执行阶段标准库，安装本次执行的回调，再调用 execute。
以下代码使用真实公开入口创建元数据环境，完整阶段切换见 [宿主测试](tests/host.rs)：

```rust
use lua_runtime_core::create_environment;
use lua_runtime_core::host::{HostDefinition, HostKind};
use serde_json::json;

let (lua, host) = create_environment(
    HostDefinition {
        kind: HostKind::Tool,
        context: json!({"task_id": "example"}),
        arguments: json!({"value": 42}),
    },
    16 * 1024 * 1024,
).unwrap();
```

`configuration_globals`、`load_program` 与 `enable_execution` 的顺序见 [环境构建实现](src/environment.rs)。
`Margatroid::enable` 固定该次执行的回调表，context getter 每次返回独立值。
`HostKind` 用于来源展示，执行能力取决于真实安装的回调。

## 从脚本能力找到实现

| Lua 入口 | 实现与示例 |
| --- | --- |
| `files:read/write/list/info/create_directory/matches` | [文件与路径实现](src/host/files.rs)、[宿主测试](tests/host.rs) |
| `data:encode/decode` 与 `data.null` | [数据桥接](src/host/data.rs)、[数据测试](tests/internal/data.rs) |
| `data:image_prepare` | [图片转换](src/host/image.rs)、[图片测试](tests/image.rs) |
| `text:search` | [正则文本查询](src/host/text.rs) |
| `process:start`、进程读写与等待 | [进程 registry](src/host/process.rs)、[进程测试](tests/internal/process.rs) |
| `http:request` | [HTTP 实现](src/host/http.rs)、[本地服务测试](tests/http.rs) |
| `mcl/agent/workspace/tools/task` | [共享命名空间](src/host.rs)，业务接入见 [Tool services](../../tool/plugin/src/services.rs) |

HTTP 返回 `status/headers/body`，正文使用原始字节；状态码交给脚本解释，请求受执行取消与本地限额约束。
`wait_any` 并发等待具名操作，返回首个完成项及其结果，并释放其余等待 Future。
共享值桥接保留空数组、空对象与 null 的身份，输入表示校验见 [值与通知测试](tests/core.rs)。

## 独立 VM 协议与资源归属

`StartDriverLuaVm` 携带 owner、workspace、program、environment 与 reply；启动回复返回 `DriverLuaVmInstance`。
`DriverLuaVm` 组件保存该实例的 ready、wake、ended 和 cancel；`WorldLuaRuntimeExt` 按完整实例发送通知。
`StopDriverLuaVm` 按实例请求结束，`DriverLuaVmExited` 报告自然退出；实际线程监督由 [plugin](../plugin/README.md) 承担。
宿主日志能力沿当前执行来源记录诊断，子进程与 HTTP 请求归本次执行 registry 管理。
各入口的精确字段见 [事件定义](src/events.rs)，通知与旧实例校验见 [core 测试](tests/core.rs)。
