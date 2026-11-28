# tool_resource_plugin

本 crate 将 `ImportTool` 转为异步文件校验，再向 owner 的 `Tools` 发布完整定义。
资源地址解析、资源种类检查和跨类别 alias 协调由调用入口完成。

## 主要对象

| 对象 | 用途 |
| --- | --- |
| `ToolResourcePlugin` | 公开插件入口；`new` / `default` 选择 update schedule |
| `ImportTool` | core 提供的导入事件，携带完整 owner、alias、路径、取消令牌及回复端 |
| `ImportToolReply` | 共享 oneshot sender，发送 `Result<(), ToolResourceError>` |
| `Tools` / `ToolEntry` | 成功导入后可查询的集合与工具定义 |

## 最小导入流程

[导入测试](tests/imports.rs) 的 `app`、`import_request`、`finish` 给出完整可执行流程：

1. 创建 `App`，依次安装 `RuntimePlugin`、`AsyncRuntimePlugin`、`ClosurePlugin`，为 `RuntimePlugin::UPDATE` 调用 `add_closure_system`，再安装 `ToolResourcePlugin`。
2. 创建 owner，准备工具目录的绝对入口路径。
3. 创建 oneshot channel，把 sender 包装为 `Arc<Mutex<Option<_>>>`，构造 `ImportTool`。
4. 调用 `world.emit_event(request)`，持续驱动 App，接收 oneshot 的最终结果。
5. 成功后借用 owner 的 `Tools`，通过 alias 取得路径、描述和 schema。

`with_schedule(schedule)` 可选择已存在的循环 schedule，并要求在该阶段注册闭包执行器。插件安装检查运行时资源、
唯一安装和 schedule；配置错误触发断言。默认 schedule 为 `RuntimePlugin::UPDATE`。

## 输入与完成语义

导入 alias 首字符为 ASCII 字母或下划线，后续字符支持 ASCII 字母、数字、下划线和短横线。
入口采用绝对路径，各路径段须为普通定位段；父目录跳转返回 `InvalidPath`。
同步文件校验规则见 [core 文档](../core/README.md)。

Handler 先认领回复端，在 `spawn_blocking` 中规范化路径并调用 `read_tool`。
异步服务等待 worker 完成，交回规范路径、描述和 schema。
结果入口再次核对完整 Entity 与取消状态，创建所需 `Tools`，再经 `insert` 登记。
导入记录的任务归属和 binding 均为 `None`；相同 alias 返回 `AliasConflict`。
成功回复发生在集合发布之后，失败保留原有条目。

## 接口参考

- [公开插件实现](src/lib.rs)：构造、schedule 配置和安装检查。
- [Handler](src/handler.rs)：校验 worker、集合发布、取消与唯一回复。
- [System](src/system.rs)、[内部类型](src/types.rs)：事件路由和异步结果类型。
- [真实异步导入测试](tests/imports.rs)：定义保存、重复 alias、取消和 Entity generation。
- [领域设计](../DESIGN.md)。
