# prompt_resource_plugin

本 crate 异步校验 `ImportPrompt` 指定的文件，并向 owner 的 `Prompts` 集合发布规范路径。
调用入口负责资源地址解析、种类检查及跨类别 alias 协调。

## 主要对象

| 对象 | 用途 |
| --- | --- |
| `PromptResourcePlugin` | 公开安装入口，配置 schedule 与读取预算 |
| `ImportPrompt` | core 提供的事件及唯一回复端 |
| `PromptReadLimits` | `with_limits` 接受的清单、正文字节预算 |
| `ImportPromptReply` | 共享 oneshot sender，发送 `Result<(), PromptResourceError>` |
| `Prompts` | 成功导入后可查询的 alias 与规范路径集合 |

## 最小导入流程

[导入测试](tests/imports.rs) 的 `app`、`import_request`、`finish` 展示可执行流程：

1. 创建 `App`，依次安装 `RuntimePlugin`、`AsyncRuntimePlugin`、`ClosurePlugin`，为 `RuntimePlugin::UPDATE` 调用 `add_closure_system`，再安装 `PromptResourcePlugin`。
2. 创建 owner，准备提示词目录的绝对入口路径与 alias。
3. 创建 oneshot channel，包装 sender 为 `Arc<Mutex<Option<_>>>`，构造 `ImportPrompt`。
4. 调用 `world.emit_event(request)`，持续驱动 App 并接收最终回复。
5. 成功后通过 `WorldPromptResourceExt::prompt_path` 取得路径，按次读取提示词。

`new` / `default` 使用 `RuntimePlugin::UPDATE`；`with_schedule` 选择已注册闭包执行器的循环 schedule，
`with_limits` 调整默认读取预算。插件安装检查运行时资源、唯一安装和 schedule，配置错误触发断言。

## 校验与发布

导入 alias 首字符为 ASCII 字母或下划线，后续支持 ASCII 字母、数字、下划线和短横线。
入口采用绝对路径，父目录跳转返回 `InvalidPath`。
Handler 认领回复端后，提交异步服务；blocking worker 规范化路径并调用 `read_prompt`。
服务等待 worker 完成，将规范路径交回 ECS。

结果入口再次检查完整 owner Entity 与取消令牌，创建所需 `Prompts` 并插入单个 alias。
同 alias 的成功结果替换旧路径，失败保留原条目；发布后发送 `Result<(), PromptResourceError>`。
正文在本次校验完成后释放，集合保存路径。后续内容读取使用 [core 入口](../core/README.md)。

## 接口参考

- [公开插件与安装检查](src/lib.rs)。
- [Handler](src/handler.rs)：blocking worker、发布、取消与回复守卫。
- [System](src/system.rs)、[内部类型](src/types.rs)：事件路由与异步结果。
- [真实异步导入测试](tests/imports.rs)：路径替换、失败保留、预算、取消和 Entity generation。
- [领域设计](../DESIGN.md)。
