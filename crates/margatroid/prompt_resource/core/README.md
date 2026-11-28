# prompt_resource_core

本 crate 提供提示词路径集合、同步文件读取和导入事件，依赖 `core_plugin` 与通用库。
读取器实际执行文件 I/O，并返回具有明确角色的 `PromptInput`。

## 主要对象

| 对象 | 用途与入口 |
| --- | --- |
| `Prompts` | `new` / `default` 创建集合；`get`、`insert`、`remove`、`iter` 管理路径 |
| `PromptInput` | 本次读取的 `role` 与 `content` |
| `PromptRole` | System、User、Assistant 角色枚举 |
| `PromptReadLimits` | 清单及正文的字节预算 |
| `WorldPromptResourceExt` | `prompt_path(owner, alias)` 按完整 Entity 借用登记路径 |
| `ImportPrompt` | owner、alias、路径、取消令牌及共享回复端 |
| `PromptResourceError` | 用 `kind`、`path`、`message` 描述失败 |

## 最小读取流程

准备包含 `prompt.toml` 和 `content.md` 的目录，然后使用同步入口：

```rust
use std::path::Path;
use prompt_resource_core::{read_prompt, PromptInput, PromptReadLimits, PromptResourceError};

fn load(entry: &Path) -> Result<PromptInput, PromptResourceError> {
    read_prompt(entry, &PromptReadLimits::default())
}
```

清单 `role` 支持 `system`、`user`、`assistant`。正文采用 UTF-8 并包含有效文本。
默认清单预算为 1 MiB，正文预算为 8 MiB，可通过 `PromptReadLimits` 分别调整。
读取器在打开文件后和读取过程中检查预算，超出时返回 `TooLarge`。
异步使用方将此同步调用安排到 blocking worker。

## 集合与文件语义

`Prompts::insert` 保存路径并返回旧值；同 alias 可替换。
`prompt_path` 检查 owner 存活性与 alias，再借用集合中的路径。
集合管理与文件校验分别通过集合方法和 `read_prompt` 完成。

`read_prompt` 每次规范化入口、解析 TOML 并读取同目录的 `content.md`。
入口和正文须为普通文件，正文真实路径须位于入口父目录内。
文件缺失、路径错误、UTF-8 错误、定义错误和读取失败分别由错误枚举表达。
`ImportPrompt` 的克隆共享同一个可认领一次的 oneshot 回复端。

## 接口参考

- [crate 导出、World 扩展及读取器](src/lib.rs)。
- [集合](src/components.rs)、[读取类型和预算](src/types.rs)。
- [事件协议](src/events.rs)、[错误枚举](src/error.rs)。
- [文件与集合测试](tests/resources.rs)：按次读取、角色、预算和 Entity generation。
- [异步导入方](../plugin/README.md)、[领域设计](../DESIGN.md)。
