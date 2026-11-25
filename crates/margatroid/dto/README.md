# DTO

DTO 提供统一 `/ws` 入口、HTTP 工作目录文件读取，以及52种 WebSocket 消息：21种请求、21种一次性响应、10种主动同步。字段和信息流见本领域 [DESIGN.md](DESIGN.md) 及独立 API 仓库。源码与测试通过不代表正在运行的 daemon 已更新。

## 请求与回复

客户端先注册连接，后续请求由 `DtoRequest` 交给 `dto_route_system`；回合中止使用 PRE_UPDATE 的 `AbortTurnControl`。领域请求沿原 oneshot 交给实际执行方，网络闭包等待原结果并返回专属 `res` 或 `result`，不另发网络完成事件。

Workspace 创建／删除分别调用对应生命周期事件；Agent 启动转为 `StartWorkspaceAgent::Existing/New`，最终调用 Driver 工具；关闭按 Agent 的父实体与任务 ID 找实际创建任务，归档等待真实清理及目录和数据库提交。网络层不准备 Agent 存储连接或直接装配实体。

`workspace.rename` 接受 address 和非空 alias；`agent.rename` 接受 workspace、持久 agent_id 和必填 nullable alias，null 清空别名。前置校验要求已注册连接和 Running Workspace，随后在受监督异步闭包及 blocking worker 中调用数据库事务，完成后回复原 oneshot；不新增业务事件，不依赖运行 Agent，不改节点、目录或 Driver 参数。数据库确认快照的 metadata.alias 与 agents 一并驱动 workspace.sync，提交成功的 result 与 sync 可能按不同顺序到达，同值不重复发布。

## 资源查询

`resource.req` 支持可选 `type`、`scope`、`name`，精确匹配并取交集。type 为 driver、tool、prompt 或 sandbox；不指定筛选时返回全部匹配条目及所有 tag，不补默认 scope 或 tag。

World 阶段只核对已注册连接并克隆装配方注入的 `address::ResourceRoots`。一次性异步闭包持有请求和 HTTP 生命周期 guard，在 blocking worker 调用 `address::query_resources`，结果由原 oneshot 返回，不需要额外 World 帧。查询不启动 Agent、执行脚本、写文件、建立缓存或监控，也不新增资源 sync。

响应包含完整资源地址、四个地址段、解析后的绝对 `path` 和可空 `tool_path`。工具的 tool_path 等于 path；Driver 有工具清单时返回该实际启动入口；提示词、沙箱或无清单的 Driver 为 null。路径遵循文件资源解析规则，允许根内入口链接，越界链接不会进入结果；资源内容合法性仍由实际导入或执行时验证。

没有匹配时成功返回空数组；目录读取、worker 或路径编码失败则返回错误并省略 resources，不以空数组伪装失败。每次查询读取当前文件系统，不提供隐式重放、模糊匹配或目录选择器。

## 探针与共享槽位

组件变化、集合修改及异步监控持有同一个 Clients Arc 槽位，按当前注册身份投递连接原有出站队列。只有实际变化推进版本，读取、重复值、失败事务及脚本输出游标变化不制造同步；不安装同步 System、不设置同步中转通道、不逐帧扫描。

| 接口 | 实际来源 | 发布内容 |
|---|---|---|
| workspace.sync | Workspace 生命周期探针与数据库确认提交 watch | 当前 Workspace 和 Agent 登记完整列表 |
| agent.sync | Agent 状态、工具、沙箱、模型公共值的统一入口 | 运行 Agent 完整列表 |
| agent.turn.sync | Agent 回合统一入口 | 当前 turn 或 null |
| agent.task.sync | 任务登记、输出、Return、错误与真实清理 | 活跃集合及完整保留窗口 |
| agent.ui.sync | 创建时明确提供的 UI 目录监控 | 稳定完整集合或错误 |
| agent.log.sync | AgentStorage 确认提交的记录水位 | 有序新增日志 |
| agent.mcl.sync | AgentStorage 确认提交的记录水位 | 有序新增审计 |
| agent.message.chunk.sync | 推理累积器接受的文本变化 | 同一 generation 的累计文本快照 |
| agent.message.full.sync | 推理终态或非生成消息真实提交 | 完整消息及对应关联 |
| model.config.sync | 实际模型配置文件来源监控 | 完整配置原文及读取状态 |

状态来源只入队轻量 StateKey，具体快照读取和编码由受监督 worker 执行；同 key 更新可合并。chunk 和持久记录保持顺序，不做状态覆盖。注册初始化交付当前状态，不重放旧 chunk、日志或审计；完整列表查询不建立订阅。

## 来源生命周期与失败

Agent 的 Installed 绑定集合观察，SourcesInstalled 绑定实际 UI、存储记录及设置文件来源。SourcesClosing 使旧来源失效，取消监控并等待在途 worker 全部真实退出。设置监控只做通用配置读取验证和诊断，不缓存正文或推断 Driver 字段，也不新增 wire 消息。

Workspace 初始化只扫描一次管理目录。数据库提交后发布 owned 登记快照，不能使用 SQLite/WAL 文件事件冒充提交，也没有旧 YAML 定义监控。只读资源查询是单次扫描，与上述长期监控独立。

来源 epoch、完整 Entity 和实际运行 instance 防止迟到结果覆盖新实例；读取闭包只持 Weak 能力，避免集合、来源和连接间强引用环。断连先移除唯一 Clients 身份，再做 World 清理；已受理业务继续由实际监督收口。

未绑定 Agent 的诊断使用既有 tracing；绑定 Agent 的日志使用实际 AgentStorage 和提交探针。普通回合取消保留同 generation 终态 full，Agent 真正关闭才退休旧生成来源。推理生成消息后来持久化不再发送第二次 full。

## HTTP 与模型配置

GET/HEAD `/{workspace}/{path}` 使用编码后的完整 Workspace 节点路径，核对实际组件后读取其 project_root 内普通文件；拒绝越界、链接、目录和特殊文件，GET 返回字节流，HEAD 仅头部。此 HTTP 接口不计入52种 WebSocket 消息，也不建立文件监控。

模型配置 req/res/sync 读取完整 TOML，update 校验后原子替换；实际推理开始时重新加载路由，不用启动快照掩盖损坏文件。来源监控只刷新安全公共投影；凭据不进入 Agent 模型视图。

当前前端源码与参考客户端由前端开发者另行适配；此领域不修改前端，也不自动迁移用户目录。
