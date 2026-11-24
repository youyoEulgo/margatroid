# Lua Runtime

lua_runtime_core 定义 VM 组件、程序、环境、通知、通用 Lua 宿主绑定及启停事件，只依赖 mecs 和通用库。lua_runtime_plugin 组合所需领域 core，运行独立嵌入 VM，不依赖其他业务 plugin。本文描述当前源码，不表示运行中的 daemon 或资源产物已经替换。

## 执行与组件

Driver 工具由工具监督启动 runner，在工具进程内运行 Lua；它不通过 WorkspaceExecutor 或 Agent 创建 System 再启动一份 VM。`StartDriverLuaVm` 是独立嵌入 VM 的入口，owner 是实际 Lua 执行宿主，不默认等同 Agent。

每次独立 VM 启动有唯一 start_id，事件及回复同时核对完整 owner Entity 和实例。DriverLuaVm 挂载于执行宿主，保存 wake、ready、ended 和取消能力。真正的 Lua 状态及非 Send Future 在本次 VM 独立线程中的当前线程 Tokio Runtime/LocalSet 运行；线程归 lua_runtime_plugin 的 VmThreads 管理，不属于 Workspace。

独立 VM 不自行创建、覆盖或移除 owner 的 ToolTasks/LuaExecutionTask。业务能力由 LuaServices 和宿主回调提供；没有实际任务能力时，任务或创建方法明确失败，不借用 owner 的其他任务伪造身份。

## 启动与停止

启动 System 校验 owner、Workspace、地址、程序与环境，挂载通知组件后启动受监督线程，再提交 ready 等待。配置可设置启动超时；ready 失败或超时会请求取消并等待真实线程收口，通过原请求 reply 返回结果，不新增启动完成业务事件。

停止请求只取一次 ended 接收端，关闭 wake 并请求取消；异步收口既等待进程/Lua 结束，也等待真实线程 join。线程 join 由独立监督持有，等待者取消不会丢弃 JoinHandle；多个等待者共享同一结果。重复停止不重建回复通道。清理失败保留组件及控制能力，不把取消或通道关闭当退出证明。

自然退出发送 DriverLuaVmExited。若没有显式停止等待者，plugin 在启动交接完成后自动提交同一线程收口，再移除 DriverLuaVm 与 VmThreads 项。通知不替代原请求 reply，也不直接修改 Agent 生命周期。

受理阶段只按真实 owner.ToolTasks 中匹配当前 execution_id 的实际任务捕获 RunContext.execution_task。只有该能力存在时才调用 LuaServices.execution_finished 等待任务创建交接与子 Agent 关闭；没有匹配任务时只关闭独立 VM 自身资源，不对 owner 其他任务执行结束通知。Driver 工具的真正任务收口由工具监督负责。

## 共享宿主环境

Driver 和工具 worker 复用 create_environment、configuration_globals、load_program、enable_execution 及 Margatroid userdata。程序使用 configure(arguments, context)/execute(arguments, context)。宿主把LuaEnvironment.globals中execution_context的arguments的原始快照传给两阶段及只读margatroid.context.arguments；省略时为空对象，不硬传nil。configure 为元数据和纯计算阶段，确认配置后再开放执行方法。HostKind 只表达来源元数据，不决定能力集合。

Lua 只暴露 margatroid 对象，公共子对象为 files、data、text、process、mcl、agent、workspace、tools、task，根方法包含 log、wait、wait_any。context getter 返回独立值，修改 Lua 表不能修改宿主持有的真实来源。业务回调由 LuaServices 转交，不在 core 引入其他业务 core，不保留全局 mcl、旧 driver:mcl 或无目标 wait 别名。

公共调用示例：

```lua
local args = margatroid.context.arguments
local created = margatroid.agent:create({
    node_name = args.node_name,
    alias = args.alias,
    project_root = actual_work_directory,
    config_path = initialization_file,
    ui_path = ui_directory,
})
local result = margatroid.mcl:execute(created.address, "EFFECT start_turn")
margatroid:wait(created.address)
```

创建方法由 tool_plugin 的宿主实现检查同 Workspace 节点及 Driver 归属，准备或复用文件，打开真实持久化连接，登记后发送 StartAgent 并等待 reply。节点、别名、工作目录来自 Driver 参数；Workspace、工具真实路径、原调用参数和创建任务来自宿主固定身份。创建的 Agent 在回复之前绑定实际任务的异步关闭能力，任务清理直接等待 AgentHandle，不通过 StopAgent 绕回父任务。

Agent 的 ready/wake 槽位属于 agent_core::Agent。MCL ready 更新 Agent 状态，wake 唤醒该槽位；公共 wait 通过 LuaServices 等待目标 AgentWakeSlot。它们不拥有 VM，也不使用已移除的 AgentWaitSlot 组件。VM 自身的 DriverWakeSlot/ready 通知只服务嵌入 VM 入口。

MCL Unit 投影为空 JSON 对象 `{}`，不使用 nil/null。其他成功结果保留结构，MCL 错误为 `{Error=...}` 值；业务调用可处理错误后继续。实际 VM 取消仍结束执行，不根据 MCL 文本猜测业务结果。

## 身份与生命周期

实体地址采用绝对节点路径，例如 `/demo/coder/reviewer`，完整 Entity/generation 和运行 instance 只在宿主持有。文件资源字符串仍采用 `type:scope/name:tag` 且 tag 必须显式非空，资源路径不是实体身份。

Agent 状态为 Starting/Running/Closing，workspace 状态独立为 Running/Deleting/Error。Agent.instance 标识本次运行及 TEMP 存储命名空间；节点名复用不恢复旧运行身份。return_result 仅交付工具原调用结果，不结束 VM 或父任务。

Lua 值桥接区分数组、对象与 null；嵌套空数组保持身份，混合键、稀疏数组、循环值和超出 Lua 表达范围的数字明确失败。require 只解析注入模块。内存、源码和指令 hook 预算沿 LuaRuntimeConfig；标准库或 HostKind 不能代替操作系统沙箱。

## 诊断与资源清理

主动 log 和加载、配置、执行、清理边界的未处理故障沿同一宿主诊断能力。真实 AgentStorage 可用时由 blocking worker 写入 agent_logs，保留可信来源、原始错误及 traceback；无 Agent 时使用既有 tracing 并遵守日志过滤，不创建独立日志库。自动故障只捕获一次，已被脚本处理的异常不会在宿主入口再次记录。

日志写入失败保留原故障，不递归记日志，也不自动写消息历史或触发推理。停止和退出等待已提交日志及进程 registry 的真实清理，随后才发 ended。没有实际任务身份时，不用静默成功的业务清理回调掩盖身份缺失。

files 保留原始字节及实际工作范围，process 子进程归属本次执行 registry，取消与退出等待真实回收。工具注册和子 Agent 的生命周期绑定由实际任务监督，不由 Lua userdata 的 Drop 伪造异步完成。

当前测试覆盖独立线程、自然退出 join、重复停止、旧实例拒绝、启动超时、CPU hook 取消、子进程回收、任务集合不被替换、MCL 错误值、值桥接及诊断持久化。
