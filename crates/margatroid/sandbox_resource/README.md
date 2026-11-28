# Sandbox Resource

沙箱资源将 JSON 策略文件登记为实体上的 alias，并维护当前选择。
开发入口分为 [core](core/README.md) 的集合、选择与读取接口，以及 [plugin](plugin/README.md) 的异步导入。

## 核心对象与文件

| 对象 | 用途 |
| --- | --- |
| `Sandboxes` | 保存 alias 到规范策略路径的映射 |
| `SelectedSandbox` | 保存当前选中的可选 alias |
| `SandboxInput` | 一次读取的 JSON 策略和规范来源路径 |
| `SandboxReadLimits` | 策略文件字节预算 |
| `SandboxObserverFactory` | 为 owner 提供列表和选择变化回调 |
| `ImportSandbox` | 导入请求与唯一回复协议 |

一个沙箱资源以 `policy.json` 为入口，文件采用 UTF-8，内容为 JSON 对象。
core 完成通用 JSON 解析，执行方负责具体后端策略 schema 和活动生命周期。
`Sandboxes` 保存路径，`SelectedSandbox` 保存 alias；活动创建方在使用时读取当前策略。
本次返回的 `SandboxInput` 由本次活动持有，文件编辑作用于后续读取。

## 最小调用流程

1. 调用入口完成资源地址解析、资源种类和跨类别 alias 检查。
2. 装配运行时、`SandboxObserverFactory` 与 `SandboxResourcePlugin`。
3. 为存活 owner 发送 `ImportSandbox`，提供合法 alias、绝对路径、取消令牌和回复端。
4. blocking worker 调用 `read_sandbox`，完成后由 ECS 登记规范路径并回复。
5. 为 owner 装配 `SelectedSandbox`，通过 `sandbox_select` 选择已登记 alias。
6. 活动创建方按登记路径调用 `read_sandbox`，取得本次执行策略。

同 alias 成功导入会替换路径。选择为 `None` 时清空当前选择，选择有效 alias 时检查集合条目。
集合和选择的实际变动经统一方法通知装配方提供的回调。

## 接口导航

- [core 使用说明](core/README.md)：读取例子、选择与观察接口。
- [plugin 使用说明](plugin/README.md)：安装前提、异步请求和完成语义。
- [读取与选择测试](core/tests/resources.rs)、[导入测试](plugin/tests/imports.rs)。
- [设计对应](DESIGN.md)：领域结构及函数行为。
