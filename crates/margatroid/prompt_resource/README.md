# Prompt Resource

提示词资源将带角色的磁盘正文登记为实体上的 alias，供使用方读取并转换为消息。
开发入口分为 [core](core/README.md) 的读取与集合接口，以及 [plugin](plugin/README.md) 的异步导入。

## 核心对象与文件结构

| 对象 | 用途 |
| --- | --- |
| `Prompts` | 保存 alias 到规范入口路径的映射 |
| `PromptInput` | 一次读取的角色与正文 |
| `PromptRole` | 明确表示 System、User、Assistant 三种角色 |
| `PromptReadLimits` | 清单和正文各自的字节预算 |
| `ImportPrompt` | 导入请求与唯一回复协议 |

一个提示词目录包含：

```text
sample/
  prompt.toml  # role = 'system'、'user' 或 'assistant'
  content.md   # UTF-8 正文，含有效文本
```

角色由清单中的 `role` 决定。core 返回独立 `PromptRole`，消息转换由调用方完成。
`Prompts` 保存入口路径；每次使用调用 `read_prompt` 取得当前文件内容。
已返回的 `PromptInput` 属于本次调用材料，后续文件编辑作用于下一次读取。

## 最小调用流程

1. 调用入口完成资源地址解析、资源种类与跨类别 alias 检查。
2. 安装运行时及 `PromptResourcePlugin`，准备 owner 与绝对入口路径。
3. 发送 `ImportPrompt`，plugin 在 blocking worker 中调用 core 的同步读取器。
4. 文件校验完成后，ECS 结果入口复核 owner 与取消状态，登记规范路径并回复。
5. 使用方通过 `prompt_path` 取路径，再调用 `read_prompt` 获取角色与正文。

同 alias 成功导入会替换路径；校验失败保留旧路径。
消息转换、Block 写入与推理由各使用领域承担。
异步调用方将同步读取安排到 blocking worker，并等待实际读取结束。

## 接口导航

- [core 使用说明](core/README.md)：重要类型、读取例子与错误。
- [plugin 使用说明](plugin/README.md)：安装、请求及发布流程。
- [读取测试](core/tests/resources.rs)、[导入测试](plugin/tests/imports.rs)：真实文件和 ECS 用例。
- [设计对应](DESIGN.md)：本领域结构及函数行为。
