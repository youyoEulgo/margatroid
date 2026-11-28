# tool_resource_core

本 crate 提供工具文件读取、实体工具集合和导入协议，依赖 `core_plugin` 与通用库。
文件 I/O 的入口是同步函数 `read_tool`；异步调用方将它安排在 blocking worker。

## 主要对象

| 对象 | 用途与入口 |
| --- | --- |
| `Tools` | `new` 创建集合，`observe` 绑定变化回调，`get` / `iter` 查询可用条目 |
| `ToolEntry` | 保存入口路径、描述、schema、任务归属和 binding，字段见 [types](src/types.rs) |
| `ToolInput` | `read_tool` 返回的临时读取材料，包含程序文本 |
| `RegistrationId` / `RegistrationBinding` | 唯一注册凭证、guard 和可用性回调 |
| `RegisterToolsRequest` | 动态工具请求；每项 `lifetime` 用 `TaskReference` 指定任务 |
| `WorldToolResourceExt` | 按完整 Entity 与 alias 借用 `tool_entry` 或 `tool_path` |
| `ImportTool` | 异步导入事件，共享可认领一次的 oneshot 回复端 |

## 最小读取流程

准备包含有效清单、schema 和 Lua 文本的工具目录，然后调用：

```rust
use std::path::Path;
use tool_resource_core::{read_tool, ToolResourceError};

fn description(entry: &Path) -> Result<String, ToolResourceError> {
    let input = read_tool(entry)?;
    Ok(input.description)
}
```

读取器规范化入口并确认普通文件，读取同目录的 `input.schema.json` 与 `main.lua`。
相邻文件的真实路径须位于入口父目录内，所有文件采用 UTF-8。
清单要求 `schema_version = 1` 和包含有效文本的 `description`；schema 要求 JSON 对象，
支持内部引用，并通过 `jsonschema` 编译校验。外部引用返回 `InvalidDefinition`。

## 登记与生命周期

`insert`、`insert_batch` 和 `insert_registered_batch` 共用整批校验入口，检查 alias、
绝对路径、描述及 schema；整批校验成功后提交集合并调用 `publish_current`。
重复 alias 返回 `AliasConflict`。`remove` 按 alias 移除，`revoke` 按 alias 与注册 ID 精确撤销。
`get` 和 `iter` 根据注册的可用性回调筛选条目，guard 随登记移除或集合销毁而释放。

动态请求通过 `agent` 选择目标，每项 `lifetime` 记录任务引用；宿主解析后生成
`ToolEntry` 的任务归属字段和 `RegistrationBinding`。具体字段以 [类型声明](src/types.rs)
及 [集合实现](src/components.rs) 为准。

## 接口参考

- [crate 导出与读取实现](src/lib.rs)：`read_tool`、World 扩展及 schema 校验。
- [导入事件](src/events.rs)、[错误类型](src/error.rs)：`kind`、`path`、`message` 和回复协议。
- [真实文件与集合测试](tests/resources.rs)：可执行用例和边界输入。
- [异步导入方](../plugin/README.md)、[领域设计](../DESIGN.md)。
