# ToolPlugin

## 用户轮次中止（已实现）

工具受理及独立 turn_cancel 监督已实现，登记原子保存 turn 与沙箱快照。无 `get_turn`；状态采用 `creating/idle/working/stopping`。中止立即清空 turn，清理期间仍为 Working，正常 finish 收口才 Idle；同步必须同时包含 turn 与 status。用户轮次中止与根 Agent shutdown、owner abort、timeout、receiver closed 不同，不统一归一为正常取消。

用户轮次中止且真实清理成功返回 `Ok(ToolExecuteResponse)`：ToolCall `failed=true`、`content="Tool call aborted."`，保留调用 ID、名称及已知 resource_id；未启动调用同用该正文。已完成结果保留；每个批次调用（含未启动项）按原顺序恰有一个结果。不伪造部分执行成功，取消不撤销已发生副作用。`CleanupFailed` 仍是 Err，保留活动，不宣称取消成功或空闲。

AbortSystem 先于 MCL 实际执行与工具 admission；共享中止事件由各消费者独立观察，不依赖 take reply 独占通知。实际受理再次核对当前 turn 与请求快照；None 或已中止旧轮次不启动 runner，返回正常取消结果。

## 职责与请求

ToolPlugin 每个 `ToolExecuteRequest` 只执行一个选定资源及匹配调用，通过唯一 oneshot 返回 `ToolExecuteResponse`。它不加载 metadata、不维护资源注册表或批次，不写历史、不执行推理，也不直接结束 turn；MCL 负责匹配、排序与结果收口；历史仅由显式 history_append 保存。

共享请求位于 `margatroid_types::tool_events`，插件重新导出：

```text
ToolExecuteRequest {
    id, agent,
    resource: CallableResource { resource_id, alias, definition: Arc<CallableDefinitionSnapshot> },
    call: ToolCall { id, tool_name, arguments },
    turn, cancel, reply
}
```

`resource` 是唯一资源快照字段，没有重复 resource_id。准备阶段消费选定 Tool metadata 的 schema、脚本路径，不按 ID 重取 latest，也不再次 global alias query。`call.tool_name` 必须等于选定 alias。归属校验要求目标 ResourceSet 存在该 ID、Agent.driver_dependencies 许可及 ImportBindings 已导入该 ID/TOOL kind；不要求旧 snapshot 与 reload 后的新 alias/定义一致。

Tool 按选定 schema 校验 JSON 参数，统一通过独立 Lua runner 执行。CallableDefinitionSnapshot 只包含 Tool；其他资源类型不能作为可执行 TOOL。

MCL `tool_call` 接收 MESSAGE array（恰好一条 assistant）+ visible TOOL array，调用只从 `assistant.tool_calls` 提取；手工调用也使用这个结构。`ToolCall` 是纯值，不存在 TOOL_CALL Block array。visible alias 冲突、重复 call ID、多条或非 assistant 在任何 submit 前失败；空 calls 返回 emptyMessage，不重复记录 assistant。

## 安装与独立 runner

安装需要 Runtime、AsyncRuntime 及相同 schedule 的通用 closure system。`ToolPlugin::default()` 提供执行限额，`with_schedule`、`with_runner_path`、`with_limits` 可配置。插件只挂一个准备 System。

```text
选定 CallableResource -> ToolExecuteRequest -> 主线程校验与作用域登记
    -> 独立 tool_runner -> 真实 cleanup -> oneshot -> MCL 收口
```

Lua 工具在独立 `tool_runner` 可执行目标执行，不在 daemon 或 Workspace Driver VM 内执行。请求经 stdin 发送结构化 JSON；stdout 只承载版本化结果协议，Lua print 和日志写 stderr。协议无效、非零退出、输出超限、启动失败及 cleanup 缺失均返回明确错误；返回字符串中的已知业务 JSON 信封按下述图片结果契约解包。

runner 默认在当前可执行文件同目录，可由 `MARGATROID_TOOL_RUNNER` 或 `with_runner_path` 指定。stdin 投递、stdout/stderr 有界排读和进程等待并行，统一受总执行预算、作用域取消、请求取消及 receiver 关闭监督。

## 沙箱快照与生命周期

Agent.sandbox 为选定 `SandboxValue { resource_id, policy: Arc<str> }`。执行登记与策略快照获取在共享作用域同一互斥门内，直接复制选定 policy，不按沙箱 ID 重取最新 ResourceSet。Tool 登记 Sandbox 活动。

有策略时通过 landstrip CLI `run -p policy -- tool_runner` 启动；后端缺失或不可用失败关闭，不退化为无沙箱。无策略的独立进程不等于安全沙箱。后端定位使用 `MARGATROID_SANDBOX_BACKEND`、PATH 或当前可执行文件同目录；继承环境过滤凭据及宿主注入变量。

入口 Future 丢弃只发出取消，独立 supervisor 保留执行所有权。Linux runner 启用 child subreaper，终止并 waitpid 回收后代（包括新 session）；可选 `MARGATROID_TOOL_CGROUP_ROOT` 使用已委派、可写且具备 cgroup.kill 的真实 cgroup。没有配置时不声称具备 cgroup 隔离。

进程、管道及临时目录真实清理后才 mark_completed，释放活动并回复。无法证明清理或 supervisor 意外退出时保留登记并 mark_cleanup_failed，不以 oneshot 关闭假称资源已释放。未启动提交丢弃 rollback；克隆重复请求只认领一个 Sender，不重复执行。

## runner 内能力

工具 VM 在脚本顶层执行前准备 arguments 和只读 context，提供 JSON 编解码与 shell，不注入 MCL、ready、wake 或 Driver 能力。context 保留 agent/resource/call ID、turn、project_root 及 temp_dir。

普通 shell 并行使用有界 stdout/stderr 管道；Linux 显式 `{pty=true}` 使用非阻塞 PTY，建立 session 和控制终端，输出合并在 stdout、stderr 为空，返回 code。非 Linux 明确拒绝 PTY，不自动降级。单次宿主超时和输出限额不得扩大总执行预算；取消、超时和后代回收沿用同一执行边界。handoff 当前固定返回 `UnsupportedHandoff`，未实现长期任务池或任务转交协议。

## 图片结果

runner结果仍为String。含`ok: boolean`与`content: string`的JSON envelope可携带`images: [{media_type, data}]`（raw base64）；共享图片校验失败明确返回ToolError，不将坏图片当作普通文本，也不在诊断中回显base64。旧纯文本输出不变。

`local/read-image`最多读取5MiB+1，按PNG/JPEG/GIF/WebP magic识别，路径支持绝对路径或项目根相对路径。通用`margatroid().base64_encode`宿主helper限制输入5MiB，以线性时间编码而不耗费纯Lua逐字节指令预算。

`ToolExecutionLimits.result_bytes`默认32MiB，限制含图片的String结果。`output_bytes`仍默认1MiB，限制旧纯文本、去除images后的JSON envelope以及shell输出；stderr诊断不提高预算。worker与父进程均检查，坏图片明确失败，不按工具身份特殊分支，也不允许通过额外字段逃避文本限额。协议管道仍有单独的有界JSON转义开销。

## snapshot 限制与回归

metadata snapshot 保留选定 alias、描述、schema 和材料路径；资源 reload 不暗改已有 Block 或请求，新 GET alias 取得新快照。脚本仍按选定路径 late read：路径对应磁盘文件可能被修改，因此本轮不承诺正文内容版本隔离，也没有 content hash 体系。

本地回归保留 worker/supervisor 取消、重复认领、真实 cleanup、subreaper、新 session 后代、并行大输出、PTY、作用域停止及输出/协议限制，并覆盖选定 metadata 在 collection reload 后仍生效、许可拒绝、alias 匹配、空对象 schema 参数校验及不支持的资源类型拒绝。完整契约和未闭合项见 [DESIGN.md](<DESIGN.md>)。
