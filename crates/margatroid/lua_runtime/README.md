# Lua Runtime：程序、宿主与执行实例

Lua Runtime 为工具 worker 和独立嵌入 VM 提供共享程序环境。
[lua_runtime_core](core/README.md) 定义值、环境、宿主绑定与生命周期协议，
[lua_runtime_plugin](plugin/README.md) 把独立 VM 接入 ECS 调度并监督线程结束。
工具调用的独立 runner 由 [Tool](../tool/README.md) 管理。

## 从五个对象开始

| 对象 | 生命周期中的角色 |
| --- | --- |
| `LuaProgram` | 保存程序源码与来源路径，供加载与诊断使用 |
| `LuaEnvironment` | 保存宿主注入的值、回调和 require 模块 |
| `Margatroid` | 把可信 context 与执行能力投影为 Lua 宿主对象 |
| `LuaServices` | 把带执行身份的业务调用交给组合层 |
| `DriverLuaVmInstance` | 以 owner 和 start_id 标识一次独立 VM 启动 |

公开类型见 [core 导出](core/src/lib.rs)，宿主对象见 [host](core/src/host.rs)。
`HostKind` 表达 Driver 或 Tool 来源；具体能力由该次执行安装的回调提供。

## 一份程序的两个阶段

程序定义 `configure(arguments, context)` 和 `execute(arguments, context)`。
宿主加载一次程序，让 configure 使用上下文元数据和纯计算生成配置，
确认 `timeout_ms` 后开放执行能力，并在同一 Lua 状态调用 execute。
两阶段参数与 `margatroid.context.arguments` 使用真实参数快照，省略参数时使用空对象。

下面展示适用于工具执行宿主的最小程序：

```lua
function configure(arguments, context)
    return { timeout_ms = 30000 }
end

function execute(arguments, context)
    local value = margatroid.data:decode("json", arguments.content)
    margatroid.task:return_result(value)
    margatroid.task:finish(context.task_id)
end
```

独立 VM 使用 `StartDriverLuaVm` 提交程序和环境，使用 `StopDriverLuaVm` 等待实例结束。
它的 ready/wake 通知与线程交接可沿 [独立 VM 测试](plugin/tests/runtime.rs) 阅读。
工具程序的回复与清理则沿 [Tool 执行测试](../tool/plugin/tests/execution.rs) 阅读。

## 宿主能力如何到达脚本

`margatroid.context` 提供来源与参数的独立值；业务身份由宿主保留的 `LuaExecutionIdentity` 决定。
文件、数据、文本、进程和 HTTP 由共享 host 模块提供，MCL、Agent、Workspace、工具注册与任务控制由回调组合。
`LuaServices::call` 携带 source、workspace、address、instance 和取消 token，组合层据此验证业务目标。
任务 list 返回当前调用 Agent 中已回复且仍活动的摘要；输出消费通过 read_output 显式推进。
完整命名空间与实际调用示例见 [core 开发入口](core/README.md)。

## 执行结束时发生什么

独立 VM 的 Lua 状态在受监督线程中运行，取消和自然退出进入统一资源收口。
进程 registry 回收本次执行的子进程，HTTP context 等待在途请求释放，日志等待已提交写入完成。
plugin 等待真实线程 join，再移除 VM 组件；与实际工具任务匹配的执行继续等待任务绑定的子资源清理。
执行来源可关联 AgentStorage 时，诊断存入该 Agent；其余执行使用现有 tracing。

## 继续阅读

- 值桥接、环境构建与宿主回调：[core](core/README.md)。
- 安装、Start/Stop、ready 与线程监督：[plugin](plugin/README.md)。
- 工具调用与后台交互：[Tool](../tool/README.md)。
- 入门组织参考：[SQLite 核心对象与调用过程](https://www.sqlite.org/cintro.html)。
- 异步执行背景：[Tokio 任务与所有权](https://tokio.rs/tokio/tutorial/spawning)。
