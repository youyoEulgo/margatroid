# tool_plugin

独立工具执行 plugin，依赖自己的 tool core、只读 Agent/Workspace 状态、工具及沙箱资源 core，以及 mecs 和通用库。只安装同步 InitToolTasks 路由和工具执行路由，不依赖其他领域 plugin，不执行 MCL 或修改 Agent turn。

`ToolPlugin::new()`、Default 使用 update schedule 和当前程序同目录的 tool_runner，支持 with_schedule、with_runner_path、with_limits；schedule仅允许UPDATE或POST_UPDATE，拒绝PRE_UPDATE及无法证明顺序的自定义schedule。安装前需要 RuntimePlugin 和 AsyncRuntimePlugin；配置校验预算并拒绝重复安装。独立二进制 tool_runner 由本 crate 提供，runner_main 是其公共启动入口。

InitToolTasks 仅在存活 Agent 上初始化空 ToolTasks，已有集合保留。执行请求在System受理时核对完整Agent Entity、Working及当前非空turn与request.turn匹配；无turn或不匹配直接返回保留call_id/alias的failed中止结果，不创建任务。工具入口、选中沙箱入口及 Workspace project_root 均固定为 owned 数据后提交监督任务，不跨 await 借用 World。

每次执行通过资源 core 在 blocking worker 重新读取清单、schema、Lua 和选中策略；按本次 schema 校验参数。worker先加载脚本并调用必需的configure(arguments, context)，通过Configured帧往返确认唯一timeout_ms存活配置（0不限时）后才开放执行能力并调用execute；总执行时间从配置确认开始，不给准备及配置阶段设置默认wallclock限时。配置阶段仍受内存、指令预算与取消约束，只允许纯计算，脚本只加载一次。ToolExecutionLimits仅提供非时间资源预算，ToolTask持有Lua配置确定的唯一存活deadline，不另设RPC或清理限时。任务持有独立取消 token，ToolTasks 与监督任务克隆同一个共享句柄。正常Return和中止强制Return共享同一锁内的returned状态及唯一reply所有权，只交付一次。回合中止先清turn，再由ToolTasks::abort_unreturned同步裁决未return任务、强制回复并请求终止；已return未finish任务保持运行。

worker 在独立进程中运行 Lua。全局 instruction hook 覆盖异步 coroutine；Lua 内存、指令、执行时间、host-call 时间、参数、结果、诊断、缓存和输入队列分别受预算约束。task_return 校验文本/图片结果并回复一次，VM 继续执行；重复 Return 记录 AlreadyReturned。task_finish 自身通过外层执行选择器退出，即使放在 pcall 内也不会继续后续 Lua 语句。自然退出而未 Return 返回 MissingReturn。

版本化双向 NDJSON 协议只在 stdout 传输帧，日志走有界 stderr。Return、Write、Replace、Read、Send 和 Finish 均通过宿主确认；Input 保留任意 JSON 值。task_read、task_send、task_finish 只使用本 Agent 的共享集合。task_receive 异步等待输入，不持有缓存锁。覆盖及 cursor/replaced 语义由 tool core 统一实现。

可信 Linux supervisor 在沙箱外充当 child subreaper；被选中时通过 landstrip run -p 启动受限 worker，后端失败不会改为无限制运行。未选中策略则直接启动 worker。MARGATROID_SANDBOX_BACKEND 可指定后端，否则从 PATH 查找 landstrip。具体策略 schema 由后端校验。

supervisor 扫描并回收包括 setsid 后代在内的进程，完成后才发送真实清理证明。宿主同时等待 supervisor、管道和临时目录清理，才完成finish并从ToolTasks移除、释放缓存，不保存finished条目或完成缓存。TaskReadResult仅含replaced/output/error；finish后read/send/finish均返回TaskMissing，这是领域错误，不是未知宿主函数。需在finish前读完输出；is_cleanup_complete仅供内部清理等待者使用，不对外暴露completed。取消只请求退出，不 abort 监督任务。清理失败记录 CleanupFailed、完成尚未回复的调用，并保留监督能力继续清理，不先标记finished再异步清理；中止时原LLM回复仍及时交付。可选 MARGATROID_TOOL_CGROUP_ROOT 使用已委派 cgroup v2 根目录加强回收。进程/PTY/shell 底层功能从旧实现迁移并集中在 handler；本 crate 不提供具体 bash 资源。

测试位于 tests，使用真实 runner 进程和资源文件，覆盖 Return 后交互、Finish、自身控制退出、跨任务访问、turn受理、唯一reply裁决、按次读取、预算、图片、超时、setsid 后代清理和后端失败关闭。
