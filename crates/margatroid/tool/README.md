# Tool：调用与任务

Tool 领域把一次工具调用组织为可回复、可交互、可清理的任务。
调用者取得 `ToolExecuteResponse`，运行中的工作由 `ToolTasks` 持有，监督器负责资源回收。
从 [tool_core](core/README.md) 阅读任务模型，从 [tool_plugin](plugin/README.md) 接入实际 Lua 工具执行。

## 先认识五个对象

| 对象 | 在一次调用中的作用 |
| --- | --- |
| `ToolExecuteRequest` | 携带调用身份、参数以及结果回复端 |
| `ToolTasks` | 保存某个执行宿主的活动任务集合 |
| `ToolTaskHandle` | 共享输出、输入通道、回复状态与清理控制 |
| `ToolConfiguration` | 保存 Lua configure 声明的任务存活时间 |
| `ToolExecuteResponse` | 把调用标识、文字、图片和失败状态交给调用者 |

任务类型及结果结构见 [公开类型](core/src/types.rs)，请求入口见 [事件定义](core/src/events.rs)。
注册工具调用使用 `Registered`，按资源清单路径执行使用 `Path`。

## 跟随一次调用

1. 调用者提交请求；plugin 取得工具程序、schema、工作目录和沙箱策略。
2. 宿主登记任务并启动独立 runner；Lua 加载程序，调用 `configure(arguments, context)`。
3. 宿主确认配置，开放执行能力，再调用 `execute(arguments, context)`。
4. Lua 用 `return_result` 交付原调用的一次回复，随后继续处理输入与输出。
5. `finish` 或程序退出进入清理；监督器等到实际资源回收后移除任务。

下面的即时工具把参数作为结果交付，并显式结束自身：

```lua
function configure(arguments, context)
    return { timeout_ms = 30000 }
end

function execute(arguments, context)
    margatroid.task:return_result(arguments)
    margatroid.task:finish(context.task_id)
end
```

完整请求、调度与结果读取见 [路径执行测试](plugin/tests/path.rs)。
后台交互可沿 [执行测试](plugin/tests/execution.rs) 中的 `return_keeps_vm_alive_across_turn_changes_and_supports_json_cache_replacement` 阅读。

## 回复之后的交互

`append_output` 追加结构化值，`replace_output` 用新值替换窗口并重置读取游标。
`read_output` 消费当前游标后的值并返回 `replaced/output/error`；相邻字符串在本次读取中拼接。
`send` 向目标 Lua 输入通道投递值，`receive` 等待当前任务输入；成功投递表示消息已经入队。
`finish` 等待目标清理，自身 finish 交由外层监督器结束执行。
`list` 返回调用 Agent 中已回复且仍活动的任务摘要，保留输出游标与替换标记。
`task-list` 资源把该摘要作为普通工具结果交付，示例见 [资源集成测试](plugin/tests/bundled.rs)。

## 按问题继续阅读

- 任务集合、窗口与清理控制：[core 开发入口](core/README.md)。
- 安装插件、选择 runner、运行策略与诊断：[plugin 开发入口](plugin/README.md)。
- Lua 宿主对象、值转换与嵌入 VM：[Lua Runtime](../lua_runtime/README.md)。
- 工具描述和输入 schema 的来源：[Tool Resource](../tool_resource/README.md)。
- 任务预算配置的应用入口：[Config](../config/README.md)。

本文采用 [SQLite C 接口导读](https://www.sqlite.org/cintro.html) 的对象、使用过程、接口导航顺序；异步监督背景可参考 [Tokio 任务教程](https://tokio.rs/tokio/tutorial/spawning)。
