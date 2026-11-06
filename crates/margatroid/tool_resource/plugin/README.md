# tool_resource_plugin

负责 `tool_resource_core::ImportTool` 的异步校验及工具记录发布，不依赖 address、Agent 或其他资源领域。调用方负责地址定位、资源类型匹配和跨类别 alias 唯一性。

公共入口 `ToolResourcePlugin::new()` 与 Default 使用 update schedule，支持 `with_schedule(schedule)`、`with_limits(ToolReadLimits)`。安装前需要 RuntimePlugin、AsyncRuntimePlugin 和目标循环 schedule；拒绝重复安装，不要求安装 ClosurePlugin。

请求先认领唯一 oneshot 回复端，校验完整 owner、identifier alias 和取消状态。alias 首字符为 ASCII 字母或下划线，其余允许 ASCII 字母、数字、下划线和短横线。路径必须为无父目录跳转的绝对路径。

私有 ImportContext、ImportOutput、ImportError 位于 types。System 仅路由 Import 请求和 Result 校验结果，Handler 提交异步服务，在 spawn_blocking 中规范化路径、调用core同步读取，保留描述及schema并丢弃本次程序正文。异步服务等待真实 JoinHandle 完成后发送 Result 并唤醒 ECS，不提前返回取消结果或脱离读取任务。

Handler 收到结果后再次核对完整 owner 和 cancel，成功时创建 `Tools` 并插入单个alias的完整ToolEntry（规范path、description、input_schema，owner_task_id和binding为None），同alias已存在则返回AliasConflict，其他条目保持不变，然后回复原 oneshot。失败保持旧条目；原 index 被复用也不会向新 Entity 发布迟到结果。不同请求的同 alias 顺序由调用入口串行协调。

导入保存路径、描述和schema，不缓存程序。执行时按记录path只读取main.lua，实际参数按Tools缓存schema校验，文件metadata变化不隐式更新记录，使用方安排 blocking worker 并等待完成。测试位于 tests，使用真实异步运行时覆盖更新、失败保留、取消、Entity generation、唯一回复、预算、自定义 schedule、路径规范化及安装约束。
