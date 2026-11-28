# tool_plugin：执行工具程序

`tool_plugin` 把工具资源、Agent/Workspace 状态和沙箱策略组合为真实工具执行。
它提供 `ToolPlugin` 与独立 `tool_runner`，使用 [tool_core](../core/README.md) 保存运行任务，
使用 [lua_runtime_core](../../lua_runtime/core/README.md) 安装共享 Lua 宿主能力。
依赖和 runner 构建目标见 [Cargo 清单](Cargo.toml)。

## 接入时需要的对象

| 对象 | 作用 |
| --- | --- |
| `ToolPlugin` | 配置执行调度、runner 路径、预算和任务观察者 |
| `ToolExecuteRequest` | 通过 `Registered` 或 `Path` 提交一次调用 |
| `ToolTasks` | 调用宿主持有的活动任务集合 |
| `ToolExecutionLimits` | 配置 VM 内存与输出窗口 |
| `TaskObserverBinder` | 任务集合初始化时安装观察能力 |
| `LuaServices` | 使用执行身份转交 MCL、Agent 与任务等业务操作 |

公开安装接口见 [lib](src/lib.rs)，业务服务安装入口为 `install_lua_services`，runner 入口为 `runner_main`。

## 跑通一次路径调用

[路径执行测试](tests/path.rs) 展示完整的最小应用：安装 `RuntimePlugin`、`AsyncRuntimePlugin`、
`ClosurePlugin` 与 closure system，再安装 `ToolPlugin`，给执行实体挂载 `ToolTasks::new()`。
应用把实际 runner 路径传入安装函数：

```rust
use core_plugin::App;
use std::path::PathBuf;
use tool_plugin::ToolPlugin;

fn install_tool_plugin(app: &mut App, runner: PathBuf) {
    app.add_plugin(ToolPlugin::new().with_runner_path(runner));
}
```

随后发送 `ToolExecuteRequest::Path { initiator, path, arguments, reply }` 并推进应用调度，
从 reply 读取结果，继续等待任务集合清空；测试同时演示真实 schema 校验。
注册入口的调用、原调用 ID 和 alias 关联见 [执行测试](tests/execution.rs)。
`ToolPlugin::new()` 选择当前程序同目录的 runner；`with_schedule` 接受 UPDATE 或 POST_UPDATE。
`with_limits` 配置预算，`with_task_observer` 配置任务观察者。

## 一次执行的所有权交接

1. System 校验执行实体和可选 turn，取得工具记录、程序路径、工作目录及选中策略的 owned 数据。
2. 注册任务并启动监督器；worker 加载 Lua，调用 `configure(arguments, context)`。
3. worker 通过 Configured 协议等待宿主确认 `timeout_ms`，然后开放能力并运行 execute。
4. Return 帧交付原调用结果；Write/Replace、输入和宿主请求继续服务活动任务。
5. 结束时监督器回收 worker、后代、管道与临时资源，再通过 core 完成移除。

协议帧经 stdout 的双向版本化 NDJSON 传输，诊断使用 stderr；相关实现入口见 [handler](src/handler.rs)。
`return_result` 保留任务存活，自身 `finish` 通过执行控制结束 Lua，结束其他任务则等待实际清理。
配置确认后的任务 deadline 覆盖执行与后续交互，零表示持续存活；预算说明见 [配置测试](tests/configuration.rs)。

## 来源身份与业务能力

工具的真实资源路径、任务实例和 binding 由宿主放入 context，参数按该工具 schema 解释。
`install_lua_services` 建立带 `LuaExecutionIdentity` 的服务边界，在 World 闭包中核对来源与目标。
动态注册项通过实际 `TaskScope` 绑定生命周期；作用域结束撤销工具定义，已受理调用继续独立收口。
Agent 创建的存储、启动回复与子 Agent 关闭由同一次任务监督，入口见 [services](src/services.rs)。
日志和执行故障沿宿主诊断能力记录：绑定 Agent 的记录使用 AgentStorage，其余执行使用 tracing。
[诊断测试](tests/diagnostics.rs) 展示来源关联、原始 traceback 保存与调用方错误结果。

## 平台与后续导航

Linux 工具监督器支持真实后代回收，选中 Landstrip 0.22.5 沙箱时使用 container 后端。

- 完整 Return、Send、Read、Finish 交互：[执行测试](tests/execution.rs)。
- 沙箱启动及安全诊断：[runner 测试](tests/runner.rs)。
- 实际文件、终端与 task-list 工具：[资源集成测试](tests/bundled.rs)。
- worker 内 HTTP 请求：[HTTP 测试](tests/http.rs)。
- VM 分配器与外部进程生命周期：[内存测试](tests/memory.rs)。
