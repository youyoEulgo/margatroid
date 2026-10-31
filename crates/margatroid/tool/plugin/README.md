# tool_plugin

独立工具执行 plugin，依赖自己的 tool core、只读 Agent/Workspace 状态、工具及沙箱资源 core，以及 mecs 和通用库。只安装同步 InitToolTasks 路由和工具执行路由，不依赖其他领域 plugin，不执行 MCL 或修改 Agent turn。

`ToolPlugin::new()`、Default 使用 update schedule 和当前程序同目录的 tool_runner，支持 with_schedule、with_runner_path、with_limits。安装前需要 RuntimePlugin 和 AsyncRuntimePlugin；配置校验预算并拒绝重复安装。独立二进制 tool_runner 由本 crate 提供，runner_main 是其公共启动入口。

InitToolTasks 仅在存活 Agent 上初始化空 ToolTasks，已有集合保留。执行请求只在受理时核对完整 Agent Entity 与 Idle/Working 状态，不要求 turn。工具入口、选中沙箱入口及 Workspace project_root 均固定为 owned 数据后提交监督任务，不跨 await 借用 World。

每次执行通过资源 core 在 blocking worker 重新读取清单、schema、Lua 和选中策略；按本次 schema 校验参数。准备时间计入执行预算。任务持有独立取消 token，ToolTasks 与监督任务克隆同一个共享句柄。原 oneshot 被释放、Return 完成以及 Agent turn 改变都不会终止任务。

worker 在独立进程中运行 Lua。全局 instruction hook 覆盖异步 coroutine；Lua 内存、指令、执行时间、host-call 时间、参数、结果、诊断、缓存和输入队列分别受预算约束。task_return 校验文本/图片结果并回复一次，VM 继续执行；重复 Return 记录 AlreadyReturned。task_finish 自身通过外层执行选择器退出，即使放在 pcall 内也不会继续后续 Lua 语句。自然退出而未 Return 返回 MissingReturn。

版本化双向 NDJSON 协议只在 stdout 传输帧，日志走有界 stderr。Return、Write、Replace、Read、Send 和 Finish 均通过宿主确认；Input 保留任意 JSON 值。task_read、task_send、task_finish 只使用本 Agent 的共享集合。task_receive 异步等待输入，不持有缓存锁。覆盖及 cursor/replaced 语义由 tool core 统一实现。

可信 Linux supervisor 在沙箱外充当 child subreaper；被选中时通过 landstrip run -p 启动受限 worker，后端失败不会改为无限制运行。未选中策略则直接启动 worker。MARGATROID_SANDBOX_BACKEND 可指定后端，否则从 PATH 查找 landstrip。具体策略 schema 由后端校验。

supervisor 扫描并回收包括 setsid 后代在内的进程，完成后才发送真实清理证明。宿主同时等待 supervisor、管道和临时目录清理，再设置 completed，保留任务及缓存。取消只请求退出，不 abort 监督任务。清理失败记录 CleanupFailed、完成尚未回复的调用，并保留监督能力继续清理，不虚报 completed。可选 MARGATROID_TOOL_CGROUP_ROOT 使用已委派 cgroup v2 根目录加强回收。进程/PTY/shell 底层功能从旧实现迁移并集中在 handler；本 crate 不提供具体 bash 资源。

测试位于 tests，使用真实 runner 进程和资源文件，覆盖 Return 后交互、Finish、自身控制退出、跨任务访问、turn/reply 独立、按次读取、预算、图片、超时、setsid 后代清理和后端失败关闭。
