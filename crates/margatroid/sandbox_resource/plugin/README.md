# sandbox_resource_plugin

本 crate 异步校验 `ImportSandbox` 指定的策略文件，并向 owner 的 `Sandboxes` 发布规范路径。
调用入口负责地址解析、资源种类检查及跨类别 alias 协调。

## 主要对象

| 对象 | 用途 |
| --- | --- |
| `SandboxResourcePlugin` | 公开插件入口，配置 schedule 与策略读取预算 |
| `ImportSandbox` | core 提供的导入事件和唯一回复协议 |
| `SandboxReadLimits` | `with_limits` 接受的策略字节预算 |
| `SandboxObserverFactory` | 装配方提供的 owner 列表和选择变化回调工厂 |
| `ImportSandboxReply` | 共享 oneshot sender，发送 `Result<(), SandboxResourceError>` |
| `Sandboxes` | 成功导入后可查询的 alias 与规范路径集合 |

## 最小导入流程

[导入测试](tests/imports.rs) 的 `app`、`import_request`、`finish` 展示真实异步流程：

1. 创建 `App`，插入 `SandboxObserverFactory`，依次安装 `RuntimePlugin`、`AsyncRuntimePlugin` 和 `ClosurePlugin`，为 `RuntimePlugin::UPDATE` 调用 `add_closure_system`。
2. 安装 `SandboxResourcePlugin`，创建 owner，准备绝对策略路径和 alias。
3. 创建 oneshot channel，包装 sender 为 `Arc<Mutex<Option<_>>>`，构造 `ImportSandbox`。
4. 调用 `world.emit_event(request)`，持续驱动 App 并接收最终结果。
5. 成功后通过 owner 的 `Sandboxes` 或 `sandbox_path` 取得登记路径。

`new` / `default` 使用 `RuntimePlugin::UPDATE`，`with_schedule` 选择已注册闭包执行器的循环 schedule，
`with_limits` 设置策略预算。插件安装检查运行时、唯一安装和 schedule，配置错误触发断言。
发布时创建 `Sandboxes` 需要 `SandboxObserverFactory` Resource；装配方在处理请求前提供它。

## 校验与完成语义

导入 alias 首字符为 ASCII 字母或下划线，后续支持 ASCII 字母、数字、下划线和短横线。
入口采用绝对路径，父目录跳转返回 `InvalidPath`。策略支持 UTF-8 JSON 对象，
读取预算及错误分类见 [core 文档](../core/README.md)。

Handler 认领回复端后提交异步服务，blocking worker 规范化路径并调用 `read_sandbox`。
服务等待 worker 完成，ECS 结果入口再次检查完整 Entity 与取消状态。
成功时取得 owner 的通用列表回调，创建所需集合，再插入单个 alias 并回复。
同 alias 成功导入会替换路径；失败保留原有条目。集合保存规范路径，策略正文属于本次校验材料。
选择与后续活动读取使用 core 的独立接口。

## 接口参考

- [公开插件及安装检查](src/lib.rs)。
- [Handler](src/handler.rs)：worker、工厂绑定、发布、取消与回复守卫。
- [System](src/system.rs)、[内部类型](src/types.rs)：事件路由与异步结果。
- [真实异步导入测试](tests/imports.rs)：路径替换、失败保留、预算和 Entity generation。
- [领域设计](../DESIGN.md)。
