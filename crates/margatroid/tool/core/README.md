# tool_core：共享任务模型

`tool_core` 提供工具请求、回复、任务集合和资源清理控制。
它依赖 mecs 的组件与事件基础设施，以及通用序列化、异步库；领域组合由 [tool_plugin](../plugin/README.md) 承担。
[依赖声明](Cargo.toml) 与 [公开导出](src/lib.rs) 是 Rust 接入的起点。

## 核心对象

| 类型 | 职责与来源 |
| --- | --- |
| `ToolTasks` | 执行宿主持有的共享组件，登记和查询活动任务 |
| `ToolTaskHandle` | 每次执行创建的共享控制句柄，持有回复、缓存、输入与取消能力 |
| `TaskReadResult` | 消费式读取返回的 `replaced/output/error` |
| `TaskListEntry` | 已回复活动任务的 `task_id/tool_name/has_unread_output/error` |
| `ToolTaskSnapshot` | 任务身份、回复状态及完整保留输出窗口的纯值投影 |
| `TaskScope` | 任务拥有的动态注册作用域，通过 `TaskBinding` 绑定撤销动作 |
| `ToolExecutionLimits` | Lua VM 内存预算与每任务输出窗口预算 |

集合实现见 [components](src/components.rs)，句柄与结果见 [types](src/types.rs)，注册作用域见 [lifetime](src/lifetime.rs)。

## 从建立集合到移除任务

1. 执行宿主建立 `ToolTasks`，创建句柄时传入该集合的 `update_context()`。
2. `ToolTaskHandle::new` 接收原调用回复端、turn 和实际 `TaskFinish` 回调，同时返回 Lua 输入接收端。
3. `register` 保存句柄；工作任务克隆句柄后共享同一输出窗口、游标和回复状态。
4. `configure_lifetime` 确认存活预算；`return_result` 消费一次回复端，任务继续存活。
5. `finish` 关闭任务作用域并请求取消，通过回调等待资源清理。
6. 监督器在子资源与工作结束后调用 `complete_cleanup`，移除对应实例并通知等待者。

下面展示集合的创建、列表及快照入口：

```rust
use tool_core::ToolTasks;

let tasks = ToolTasks::new();
assert!(tasks.is_accepting());
assert!(tasks.list_returned().is_empty());
assert!(tasks.snapshot().is_empty());
```

包含真实句柄、回复端和 finish 回调的完整构造见 [任务测试](tests/tasks.rs) 的 `handle_for`，
异步子资源与补偿清理见 [通用任务测试](tests/generic_tasks.rs)。

## 输出、输入与查询

`write` 追加值，`replace` 替换窗口并重置游标；`read` 推进共享游标并消费替换标记。
读取时相邻字符串拼接，结构化值保留顺序；窗口超限时滚出旧值，超长字符串保留合法 UTF-8 尾部。
`send` 将值写入输入队列，接收协议由实际执行程序解释。
`list` 取得活动句柄；`list_returned` 取得已回复活动任务摘要，并保留缓存正文、游标与替换标记。
`snapshot` 返回完整保留窗口的纯值，适合任务展示；`snapshot_source` 为观察者提供版本与投影。
清理完成后，按原任务 id 执行读取、发送及结束请求返回 `TaskMissing`。

## 回复和生命周期接口

`abort_unreturned(turn)` 为匹配回合且等待回复的调用交付中止结果，并请求取消；已回复任务继续按自身生命周期运行。
正常回复与中止回复通过共享状态裁决唯一交付，重复回复产生 `AlreadyReturned`。
`ToolConfiguration.timeout_ms` 的正整数表示配置确认后的存活毫秒数，零表示持续存活至显式结束或自然退出。
`observe` 与 `TaskUpdateContext` 把集合实际变化交给观察者，读游标和输入消息沿各自操作推进。

- 请求和初始化事件：[events](src/events.rs)。
- 文字与图片结果转换：[`parse_tool_result`](src/lib.rs) 及 [结果测试](tests/results.rs)。
- 任务归属与快照：[归属测试](tests/ownership.rs)。
- 配置预算与截止时间：[配置测试](tests/configuration.rs)。
- 注册与清理次序：[生命周期测试](tests/lifetime.rs)。
