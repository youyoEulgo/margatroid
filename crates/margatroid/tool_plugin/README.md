# ToolPlugin

## 职责与请求

ToolPlugin 每个 `ToolExecuteRequest` 只执行一个选定资源及匹配调用，通过唯一 oneshot 返回 `ToolExecuteResponse`。它不加载 metadata、不维护资源注册表或批次，不写历史、不执行推理，也不直接结束 turn；MCL 负责匹配、排序、结果收口及历史。

共享请求位于 `margatroid_types::tool_events`，插件重新导出：

```text
ToolExecuteRequest {
    id, agent,
    resource: CallableResource { resource_id, alias, definition: Arc<CallableDefinitionSnapshot> },
    call: ToolCall { id, tool_name, arguments },
    turn, cancel, reply
}
```

`resource` 是唯一资源快照字段，没有重复 resource_id。准备阶段消费选定 Tool/Skill metadata 的 schema、脚本/正文路径，不按 ID 重取 latest，也不再次 global alias query。`call.tool_name` 必须等于选定 alias。归属校验要求目标 ResourceSet 存在该 ID、Agent.image_dependencies 许可及 ImportBindings 已导入该 ID/TOOL kind；不要求旧 snapshot 与 reload 后的新 alias/定义一致。

Tool 按选定 schema 校验 JSON 参数；Skill 固定 schema 为 `{ "type":"object", "properties":{}, "additionalProperties":false }`，只允许 `{}`，有界读取选定正文并返回工具结果。Hook 可导入 TOOL、可提供推理模型定义，但执行明确返回 `UnsupportedHook`，不启动普通 Lua runner；Driver 触发器执行路线仍未闭合。prompt/sandbox 不是可执行 TOOL。

MCL `tool_call` 接收 MESSAGE array（恰好一条 assistant）+ visible TOOL array，调用只从 `assistant.tool_calls` 提取；手工调用也使用这个结构。`ToolCall` 是纯值，不存在 TOOL_CALL Block array。visible alias 冲突、重复 call ID、多条或非 assistant 在任何 submit 前失败；空 calls 返回 emptyMessage，不重复记录 assistant。

## 安装与独立 runner

安装需要 Runtime、AsyncRuntime 及相同 schedule 的通用 closure system。`ToolPlugin::default()` 提供执行限额，`with_schedule`、`with_runner_path`、`with_limits` 可配置。插件只挂一个准备 System。

```text
选定 CallableResource -> ToolExecuteRequest -> 主线程校验与作用域登记
    -> 独立 tool_runner / Skill 有界读取 -> 真实 cleanup -> oneshot -> MCL 收口
```

Lua 工具在独立 `tool_runner` 可执行目标执行，不在 daemon 或 Workspace Driver VM 内执行。请求经 stdin 发送结构化 JSON；stdout 只承载版本化结果协议，Lua print 和日志写 stderr。协议无效、非零退出、输出超限、启动失败及 cleanup 缺失均返回明确错误，不隐式解包返回字符串中的业务 JSON 信封。

runner 默认在当前可执行文件同目录，可由 `MARGATROID_TOOL_RUNNER` 或 `with_runner_path` 指定。stdin 投递、stdout/stderr 有界排读和进程等待并行，统一受总执行预算、作用域取消、请求取消及 receiver 关闭监督。

## 沙箱快照与生命周期

Agent.sandbox 为选定 `SandboxValue { resource_id, policy: Arc<str> }`。执行登记与策略快照获取在共享作用域同一互斥门内，直接复制选定 policy，不按沙箱 ID 重取最新 ResourceSet。Tool 登记 Sandbox 活动，Skill 登记 Other 活动。

有策略时通过 landstrip CLI `run -p policy -- tool_runner` 启动；后端缺失或不可用失败关闭，不退化为无沙箱。无策略的独立进程不等于安全沙箱。后端定位使用 `MARGATROID_SANDBOX_BACKEND`、PATH 或当前可执行文件同目录；继承环境过滤凭据及宿主注入变量。

入口 Future 丢弃只发出取消，独立 supervisor 保留执行所有权。Linux runner 启用 child subreaper，终止并 waitpid 回收后代（包括新 session）；可选 `MARGATROID_TOOL_CGROUP_ROOT` 使用已委派、可写且具备 cgroup.kill 的真实 cgroup。没有配置时不声称具备 cgroup 隔离。

进程、管道及临时目录真实清理后才 mark_completed，释放活动并回复。无法证明清理或 supervisor 意外退出时保留登记并 mark_cleanup_failed，不以 oneshot 关闭假称资源已释放。未启动提交丢弃 rollback；克隆重复请求只认领一个 Sender，不重复执行。

## runner 内能力

工具 VM 在脚本顶层执行前准备 arguments 和只读 context，提供 JSON 编解码与 shell，不注入 MCL、ready、wake 或 Driver 能力。context 保留 agent/resource/call ID、turn、project_root 及 temp_dir。

普通 shell 并行使用有界 stdout/stderr 管道；Linux 显式 `{pty=true}` 使用非阻塞 PTY，建立 session 和控制终端，输出合并在 stdout、stderr 为空，返回 code。非 Linux 明确拒绝 PTY，不自动降级。单次宿主超时和输出限额不得扩大总执行预算；取消、超时和后代回收沿用同一执行边界。handoff 当前固定返回 `UnsupportedHandoff`，未实现长期任务池或任务转交协议。

## snapshot 限制与回归

metadata snapshot 保留选定 alias、描述、schema 和材料路径；资源 reload 不暗改已有 Block 或请求，新 GET alias 取得新快照。脚本和 Skill 正文仍按选定路径 late read：路径对应磁盘文件可能被修改，因此本轮不承诺正文内容版本隔离，也没有 content hash 体系。

本地回归保留 worker/supervisor 取消、重复认领、真实 cleanup、subreaper、新 session 后代、并行大输出、PTY、作用域停止及输出/协议限制，并覆盖选定 metadata 在 collection reload 后仍生效、许可拒绝、alias 匹配、Skill 空对象及 UnsupportedHook。完整契约和未闭合项见 [DESIGN-NEXT.md](<DESIGN-NEXT.md>)；历史设计不作为当前 API。
