# Margatroid V3 路线图

状态：API 收口与阶段 3.5 已完成，正在设计阶段 4 资源库 API

路线图只记录工作顺序与验收条件，不重复 API 和产品设计。架构见
[V3-DESIGN.md](V3-DESIGN.md)。

## 发布目标

v0.1 是可以独立安装和使用的本地多 Agent 编排产品：

```text
安装 CLI + daemon
-> 准备 AgentImage / Skill / Workflow
-> 编写 margatroid-workspace.yaml
-> margatroid workspace up
-> 查看状态与日志
-> 与 manager 对话
-> stop/start/restart/down
-> 重启 daemon 后恢复状态与记忆
```

## 已完成基线

### 阶段 0：V3 入口切换

- CLI 与 daemon 使用新的 apps 入口。
- 旧 runtime 移入 legacy，不参与 workspace。
- 默认端口为 3939。

### 阶段 1：产品协议

- protocol crate 独立于 ECS 和 HTTP。
- Workspace、Request、Task、Error DTO 第一版完成。
- ID、schema version 和 JSON shape 有测试。

### 阶段 2：mecs 基础设施

- 同步 core、运行循环、外部事件和异步 runtime。
- tracing、HTTP、signal、terminal input Plugin。
- 默认 daemon Plugin 组合。

### 阶段 3：Compose 工具链

- YAML authoring schema。
- 项目路径、WorkspaceSpec 和资源引用解析。
- YAML 上限、路径和诊断测试。

阶段 3 实现中已有的 WorkspaceBundle 和资源打包代码不再是目标 API；它们在阶段 4/5
随 daemon 本地资源边界一起简化，不为旧实现保留兼容层。

## API 收口（已完成）

目标：在继续业务实现前缩小并统一公开面。

- 以 KISS 规则重写 mecs 与 Margatroid API 文档。
- core 隐藏 raw Schedule、报告和重复 Resource wrapper。
- app runtime 将五阶段 shutdown 收敛为逆序回调。
- 基础设施 Plugin 去掉重复 Handle/State/Event 入口。
- 审查现有业务 Plugin，记录仍需迁移为 Command/Result、只读 Resource 的边界。
- 标记旧 types/providers/assets/paths/sandbox 的迁移边界。
- 所有 workspace test 和 doctest 通过。

完成条件：代码公开导出与两份 API 文档一致，下一阶段不再依赖旧业务事件命名扩张。

阶段结束后先提交经过完整验证的 API 与文档检查点，再修改活动依赖图。

## 阶段 3.5：活动边界清理（已完成）

在新增业务能力前，让实际依赖图符合已经确定的职责边界：

- 默认 daemon 只组合稳定且启动所需的基础设施 Plugin。
- 移除 Config、EventBus、LLM、Sandbox、Skill 等未完成业务 Plugin 的默认安装。
- ServerPlugin 收敛为 HTTP DTO 与业务 Command/Result 之间的适配层。
- 明确 types、providers、assets、paths、sandbox 的内部依赖、迁移或 legacy 去向。
- 清除旧 Agent、Workspace、Compose 和 Provider 概念的通配公开导出。
- 只保留后续 Agent 执行确实需要的最小 LLM provider 边界，不提前实现 Agent runtime。
- 使用 `cargo metadata` 和公开导出检查活动依赖图。

完成条件：默认 daemon 不宣称尚未实现的业务能力；活动 crate 不再向新业务代码暴露旧 V2
模型；代码依赖方向与 API 文档一致。

## 下一阶段：阶段 4 资源库

先冻结独立资源库 API，再实现存储：

- 公开写入口统一为 ResourceCommand / ResourceResult。
- ResourceCatalog 只提供 AgentImage、Skill、Workflow 查询。
- 支持从 daemon 可见的本地目录安装带作用域逻辑名称的资源。
- daemon 校验源目录、文件类型、symlink、路径和大小，并自行计算 digest。
- 为可编辑资源保留稳定 ResourceId，Agent 启动时生成不可变运行快照。
- 主目录资源采用原子持久化，并能识别损坏数据。

不在本阶段处理 Workspace 生命周期、AgentInstance 或记忆目录。
Provider 配置及 secret 由 LlmPlugin 独立拥有，不进入目录包资源库。

完成条件：三类资源可以从本地目录独立安装、查询和安全删除；CLI 不读取或
打包资源正文；重启后结果一致；公开 API 不暴露存储 Record、数据库连接或可写注册表。

## 阶段 5：WorkspacePlugin

在资源库边界稳定后实现 Workspace 状态机：

- `workspace up` 路径在 daemon 内编译为 WorkspaceSpec 与规范化项目根目录。
- 公开写入口统一为 WorkspaceCommand / WorkspaceResult。
- Workspaces Resource 只提供运行状态查询。
- daemon 解析项目级 Skill / Workflow 并生成不可变运行快照。
- 登记和释放 Workspace 使用的资源条目与快照 digest，阻止仍被使用的资源误删。
- AgentInstance Entity 映射。
- 项目级默认 Memory 路径。
- 原子持久化和进程重启恢复。

完成条件：多个 Workspace 相互隔离，状态转换可重复执行，损坏数据不会导致部分恢复，
Workspace 删除不会隐式删除共享资源或记忆。

## 阶段 6：MemoryPlugin

- 每个 AgentInstance 独立 SQLite 记忆库。
- 默认项目级路径和显式 volume 覆盖。
- 生命周期、迁移、并发和损坏恢复。
- 不把数据库连接或 SQL 细节暴露给其他 Plugin。

完成条件：重启 Workspace 后记忆保留，不同 Workspace 的同名 Agent 不串数据。

## 阶段 7：SkillPlugin 与 WorkflowPlugin

- 统一目录包加载。
- 镜像内、项目级、主目录的作用域覆盖。
- Workflow 依赖清单和 Skill 依赖检查。
- 可扩展节点注册接口。
- 第一批顺序、条件、提示词、Skill 调用节点。

完成条件：缺失依赖在启动前给出确定诊断，新节点无需修改 core。

## 阶段 8：Agent 执行

- AgentImage -> AgentInstance 启动快照。
- Provider 配置、secret 存储和 LlmProviders 只读查询。
- manager 入口和 Agent 间委派。
- LLM 流式结果、工具调用、Sandbox 和 Memory 接入。
- Request/Task 状态机。
- timeout、cancel、失败传播和并发限制。

完成条件：真实模型端到端任务通过，失败不会卡住 Workspace 或泄漏任务。

## 阶段 9：业务 HTTP API

- Workspace、资源库、Request、Task endpoint。
- SSE/WebSocket 日志与产品事件流。
- 认证、请求上限、错误 DTO 和 graceful shutdown。
- ServerPlugin 只做 HTTP DTO 与业务 Command/Result 适配。

完成条件：CLI 只发送本地路径和控制命令，不读写 daemon 权威数据；CLI 退出或重连不会
破坏运行状态。

## 阶段 10：Docker-like CLI

- workspace up/down/start/stop/restart/ps。
- run 单 AgentImage。
- agent/skill/workflow 本地路径命令与资源库查询。
- 默认附着日志、`-d` 后台运行。
- 项目发现、退出码、结构化输出和 TUI/对话入口。

完成条件：新用户只需 Workspace 文件即可完成完整运行流程。

## 阶段 11：生产硬化

- daemon 单实例锁与权限。
- 崩溃恢复、幂等命令和持久化迁移。
- 背压、资源限额、日志轮转和敏感信息审计。
- Linux/macOS 安装、升级和卸载流程。
- 真实 LLM、HTTP、文件系统和长时间运行测试。

## 阶段 12：独立发布

- mecs crate 命名、README、示例和 crate-level docs。
- Margatroid CLI/daemon 发布包。
- 版本策略、变更日志、许可证与供应链审计。
- 冻结 v0.1 协议和迁移说明。

## 执行规则

1. 每阶段先冻结最小 API，再实现。
2. 不为未来猜测提前增加公开类型。
3. 新能力先证明不能由现有组合表达。
4. 旧 API 只在当前阶段确有调用者时迁移，不保留无用户兼容层。
5. 每阶段结束先审查职责边界、稳定性、安全和可维护性，再提交。
