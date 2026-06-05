# TODO

按优先级排列。

- [x] memory.rs: std::sync::Mutex——经评估，短锁无 await 跨越无竞争，不需要改
- [ ] member.rs: 工具执行函数用 String 编码成功/失败，改为 Result<String, ToolError>
- [ ] member.rs: execute_finish 中重复 chain_snapshot()，合并为一次
- [ ] workspace: Arc<AtomicBool> 关停信号改为 CancellationToken，与 bridge 一致
- [ ] providers: OpenRouterError 与 anyhow 双路径合并
- [ ] board.rs: broadcast::channel(32) 硬编码，提取为常量或配置
- [ ] memory.rs: Worklog/PersonalMemory trait 只有一个实现，考虑简化
- [ ] 补充测试: server handlers, runtime member/workspace/client 核心路径
- [ ] 前端: 验证流式修复，完善 member_status 展示
- [ ] 多 workspace 并发支持
