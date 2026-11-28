# sandbox_resource_core

本 crate 提供沙箱路径集合、当前选择、通用变化回调、同步策略读取及导入协议。
依赖 `core_plugin` 与通用库；文件 I/O 由 `read_sandbox` 实际完成。

## 主要对象

| 对象 | 用途与入口 |
| --- | --- |
| `Sandboxes` | `new(updates)` 创建集合，`get`、`insert`、`remove`、`iter` 管理路径 |
| `SelectedSandbox` | `new(alias, updates)` 创建选择，`set` 修改可选 alias |
| `SandboxObserverFactory` | `for_owner` 生成列表与选择变化回调 |
| `SandboxInput` | 本次读取的 JSON `policy` 与规范 `origin` |
| `SandboxReadLimits` | 策略字节预算，默认 8 MiB |
| `WorldSandboxResourceExt` | `sandbox_path` 查询路径，`sandbox_select` 验证并提交选择 |
| `ImportSandbox` | 携带 owner、alias、路径、取消令牌和共享回复端的事件 |

## 最小读取流程

准备 UTF-8 JSON 对象策略文件，然后调用同步入口：

```rust
use std::path::Path;
use sandbox_resource_core::{read_sandbox, SandboxInput, SandboxReadLimits, SandboxResourceError};

fn load(entry: &Path) -> Result<SandboxInput, SandboxResourceError> {
    read_sandbox(entry, &SandboxReadLimits::default())
}
```

读取器规范化入口并确认普通文件，在打开后和读取过程中检查预算。
超出预算返回 `TooLarge`，UTF-8、JSON 解析及对象类型错误由错误枚举表达。
具体执行后端的策略字段校验由使用方完成。
异步使用方将该同步调用安排到 blocking worker，并等待实际完成。

## 集合、选择与回调

`Sandboxes::insert` 返回旧路径，同 alias 可替换；路径实际变化或成功删除时发布当前列表。
`SelectedSandbox::set` 在 alias 实际变化后发布当前选择。
`publish_current` 可由装配方显式发布当前状态。
工厂接收 `Entity`，返回 `SandboxChanged` 与 `SandboxSelectionChanged` 两个通用 Arc 回调。

`sandbox_select` 要求 owner 存活且已装配 `SelectedSandbox`；`Some(alias)` 先验证集合条目，
`None` 清空选择。`sandbox_path` 借用登记路径，`read_sandbox` 取得当前文件内容。
两者分别承担集合查找与文件读取，便于调用方确定读取时机。

## 接口参考

- [crate 导出、读取器、World 扩展与观察工厂](src/lib.rs)。
- [集合和选择组件](src/components.rs)、[输入与预算](src/types.rs)。
- [导入事件](src/events.rs)、[错误类型](src/error.rs)：`kind`、`path`、`message`。
- [真实文件与选择测试](tests/resources.rs)。
- [异步导入方](../plugin/README.md)、[领域设计](../DESIGN.md)。
