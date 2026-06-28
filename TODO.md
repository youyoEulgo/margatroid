# TODO

按优先级排列。

- [x] memory.rs: std::sync::Mutex——经评估，短锁无 await 跨越无竞争，不需要改
- [x] member.rs: 工具执行函数改为 ToolResult struct，失败时 is_error 阻止退出
- [x] member.rs: execute_finish 中重复 chain_snapshot() 合并为一次
- [x] workspace: 关停信号改为 CancellationToken，与 bridge 统一
- [x] providers: OpenRouterError 与 anyhow 双路径合并
- [x] board.rs: broadcast::channel(32) 提取为常量 CHANNEL_CAPACITY
- [x] memory.rs: Worklog/PersonalMemory trait 只有一个实现，删除 trait
- [x] 补充测试: merge_deltas, format_args_json, parse_retry, request_message_from_choice (8→22)
- [x] 后端事件格式规范化: EventPayload→EventMetadata, flatten 移除, tag→untagged, 单 EventBus 通道
- [x] 前端事件系统清理: 统一类型定义, 5 handler, 删 streamEvents/DONE/taskEs, 实时 streaming
- [ ] 多 workspace 并发支持
- [ ] runtime → runtime_v2 完全迁移，消除双写
