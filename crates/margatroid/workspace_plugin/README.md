# WorkspacePlugin

`WorkspacePlugin` 接收 Compose 生成的 `WorkspaceDefinition`，负责 Workspace 及其 Agent 的创建、消息路由、
MCL 命令转发和停止收敛。每个 Workspace 与 Agent 都获得全局唯一的资源地址，例如 `workspace:demo` 与
`agent:demo/coder:latest`；该地址用于跨 Plugin 和 API 引用，ECS Entity 只在后端内部使用。

创建与停止完全由事件驱动。StartWorkspace 创建独立执行线程和 Workspace，按驱动归并 LoadAgentDriver；成功加载后分别发送 StartAgent。全部初始请求收口后进入 Running，成员失败不阻断工作区。成功成员保存在 agents，失败诊断保存在 member_failures，不维护重复生命周期表。

StopWorkspace 只受理 Running，关闭提交门并等待已受理的动态创建以及所有 StopAgentResult，然后异步等待线程退出，最后回收 Workspace。成员创建失败不使停止失败；真正成员清理或线程关闭失败保留 Failed 实例。

## Workspace 成员启动失败隔离契约

Workspace 自身定义校验、创建或执行线程创建失败才返回 StartWorkspaceResult::Err；创建前失败不占用新地址，重复地址保留旧实例并报告其 ID、状态、成功成员数及失败成员数，不覆盖或自动附着。初始驱动和成员请求全部结束后，即使全部成员失败也进入 Running 并返回 Ok，成员失败不得关闭提交门。

Workspace 保存 member_failures: BTreeMap<ResourceID, WorkspaceMemberFailure>；失败值包含 driver 与 error，工作区身份由所属组件提供。这是持久到该工作区停止的失败诊断，不是重复 Agent 生命周期表。每个失败成员产生 WorkspaceMemberStartFailed，携带 workspace Entity、workspace_id、agent、driver、error；共享驱动失败按成员展开，动态创建失败同样记录并通知，成功加入清除该成员旧失败。Stopping 中已受理动态创建失败只结束等待，不污染健康工作区的停止结果。

WorkspaceInfoDto 新增 member_failures: Vec<WorkspaceMemberFailureDto>，各项 agent、driver 为 ResourceIDDto，error 为 String；serde 缺省为空、空列表省略。workspace.started、state.sync、state.query 均投影该唯一失败源。DTO 按配置 logs 目标逐成员发送包含工作区、成员、驱动和原始错误的日志。manager 不可用时路由返回明确 manager unavailable 诊断，不暗中切换成员。客户端收到带失败的 workspace.started 仍保持连接。
