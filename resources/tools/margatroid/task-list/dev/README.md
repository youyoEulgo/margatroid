# task_list

无参数工具，列出真实调用Agent当前已经return、尚未finish/完成清理的工具任务。模型输入为 `{}`；不接受Agent地址、过滤器或Task ID，不跨Agent列举。

```json
{
  "tasks": [
    {
      "task_id": "task-123",
      "tool_name": "bash",
      "has_unread_output": true,
      "error": null
    }
  ]
}
```

`tool_name`是创建该任务时实际调用的工具别名，例如bash、mcp_register、mcp_update或subagent，不是当前资源清单名称。列表按task_id稳定排序；空集合为 `{"tasks":[]}`。未return任务和完成清理的任务不列出，列表调用自身在读取时未return，也不出现在结果。

只读取轻量任务状态，不复制缓存正文、调用参数或完整快照，不推进共享读取cursor、不清除replaced、不发布任务变更。未读标记为 `replaced || cursor < output.len()`，因此空缓存替换也可通知调用方清空旧显示。`false`不表示任务空闲或结束；列表也不推断进程状态。

使用task_read读取缓存输出，task_send交互，task_finish主动结束。error为当前安全工具错误对象或null，不将返回结果、MCP凭据、原调用参数加入列表。任务可能在列表返回后结束，后续操作按实际TaskMissing处理。

宿主提供通用只读 `margatroid.task:list()`，Worker和独立Lua执行共享入口，执行阶段才可用。工具仍沿现有return_result/finish生命周期，不新增MCL语法、网络API、同步事件、任务缓存或持久配置。

Assistant资源模板默认导入 `tool:margatroid/task-list:dev AS task_list`；已有Agent持久初始化配置不自动覆盖，需要按原IMPORT TOOL方式导入。
