# Margatroid V3 产品路线图

状态：执行基线草案

## 1. 暂定发布目标

Margatroid v0.1 是一个可安装、可配置、可长期运行的 CLI-first 多 Agent 工作流产品；
同时发布经过 Margatroid 实际使用验证的 mecs 0.1 基础设施 crates。

v0.1 的完整用户路径：

```text
安装 margatroidd + margatroid
→ 配置 LLM provider
→ 启动 daemon
→ margatroid up -f compose.toml
→ 提交 prompt
→ 程序按确定性 workflow DAG 调度 Agent
→ 实时查看进度和结果
→ 重启 daemon 后仍能查询任务和历史
→ 停止 workspace / daemon
```

v0.1 不包含：

- Web UI
- 分布式或多节点运行
- 动态库形式的运行时 Plugin 加载
- 企业级认证和多租户
- V1/V2 兼容层
- MCP 和 bridge 产品集成
- Windows 正式支持

这些能力进入 v0.2 或更晚版本，不阻塞 v0.1。

## 2. 当前状态

已完成：

- mecs core、同步 Schedule、Event、Resource、Query 和 Plugin。
- App runtime、Async runtime、日志、HTTP 和外部事件入口。
- LLM、Sandbox、Skill、EventBus、Config 的第一版独立事件链。
- ECS daemon 正式入口和纯 HTTP CLI。
- legacy 代码隔离。

尚未完成：

- CLI/daemon 共享产品协议。
- workspace、member、workflow、memory 四个核心业务 Plugin。
- 持久化任务状态与重启恢复。
- 完整业务 HTTP API 与 CLI。
- 进程信号和优雅关闭。
- 发布元数据、CI、许可证和安装产物。

## 3. 关键依赖顺序

```text
protocol
   ↓
workspace ──→ memory
   ↓           ↓
workflow ──→ member / agent execution
   ↓
server API ──→ CLI
   ↓
hardening ──→ packaging ──→ v0.1
```

不得越过依赖顺序同时铺开多个核心业务 Plugin。每个阶段依次完成 API 设计、实现、
审查、文档和提交，再进入下一阶段。

## 4. 阶段 0：收口当前基线

工作内容：

- 提交 Margatroid 产品默认端口 `3939`。
- 删除 `ServerPlugin` 中未使用的 prompt/HTTP 占位 Event。
- 将本路线图作为正式跟踪文档。
- 在 CI 中禁止正式 workspace 重新依赖 `legacy/`。

验收门槛：

- workspace 无非预期修改。
- 完整测试、格式检查和 Clippy 通过。
- 正式入口和依赖图中只有 V3 代码。

## 5. 阶段 1：稳定产品协议

新增 `margatroid_protocol` crate，负责：

- 定义 `WorkspaceId`、`RequestId`、`TaskId`、`AgentId`。
- 定义 workspace、prompt、task 和 result DTO。
- 定义稳定错误码和 API version。
- 定义任务状态机：Queued、Running、Waiting、Completed、Failed、Cancelled。
- 定义配置 schema version 和兼容策略。
- 让 CLI 与 daemon 共同依赖协议 crate，不互相依赖实现。

验收门槛：

- 协议类型具有双向 serde 和 JSON shape 测试。
- protocol 不依赖 ECS、Axum、CLI 或 daemon。
- 关联关系只依赖稳定 ID，不依赖事件队列顺序。

## 6. 阶段 2：进程生命周期

工作内容：

- 实现 `SignalPlugin`，将 SIGINT/SIGTERM 转换为 `AppControl::shutdown()`。
- 明确 Starting、Ready、Draining、Stopped 状态。
- 定义环境变量、配置文件和 CLI 参数的优先级。
- 定义 daemon 单实例、数据目录和文件权限规则。
- 按顺序停止 HTTP listener、在途请求、异步任务和持久化层。

验收门槛：

- Ctrl-C 和 SIGTERM 均能确定退出。
- 没有遗留线程或未 join worker。
- `/ready` 只在必要依赖初始化完成后成功。

## 7. 阶段 3：WorkspacePlugin

工作内容：

- 使用 `WorkspaceRegistry` 管理多个 workspace。
- 将 compose 解析为稳定 `WorkspaceSpec`。
- 校验 manager、Agent、Provider、Skill 和 Workflow 引用。
- 实现 create、start、stop、delete、list 生命周期 Event。
- 使用 Entity 表示 Agent 实例，差异通过 Component 表达。
- 新业务中不再使用 Manager/Member/User 特殊身份分支。

验收门槛：

- 不连接真实 LLM 即可创建、列出、停止和删除多个 workspace。
- 重复名称、无 manager、无效引用产生结构化失败 Event。
- workspace 之间的 Entity、配置和事件不串扰。

## 8. 阶段 4：MemoryPlugin

工作内容：

- 设计 SQLite schema、migration 和事务边界。
- 持久化 workspace 元数据、任务、workflow 状态和 worklog。
- 隔离 Agent 私有 conversation 和 personal memory。
- 让 accepted request 在返回 `202` 前形成可恢复记录。
- 定义保留、清理、备份和数据库损坏恢复策略。

验收门槛：

- daemon 重启后 workspace 和未完成任务可恢复。
- migration 可以从每个已发布 schema version 升级。
- manager 可读共享 worklog，但不能读取其他 Agent 的私有 memory。

## 9. 阶段 5：WorkflowPlugin

工作内容：

- 解析并验证确定性 DAG。
- 支持 delegate、parallel、condition、return。
- 实现依赖调度、并发上限、timeout、retry 和 cancel。
- 每次状态转换产生结构化 Event 并持久化。
- 流程由程序控制，LLM 只执行 workflow 节点。

验收门槛：

- 使用 fake executor 覆盖串行、并行、分支和汇合。
- 覆盖失败、重试、超时、取消和重启恢复。
- 无循环依赖、悬空引用或不可达节点可以进入运行态。

## 10. 阶段 6：MemberPlugin 与 Agent 执行

工作内容：

- 定义 Soul、SkillSet、ProviderConfig、TaskContext 等 Component。
- 实现 prompt 组装、上下文裁剪和 token budget。
- 将 Workflow delegate 节点转换为 `LlmRequest`。
- 通过显式 Tool Registry 转换 Sandbox 和 Skill 请求。
- 将 LLM stream、领域失败、取消和 usage 转换为业务 Event。
- coordinator 只是普通 Agent 配置，不拥有框架特权。

验收门槛：

- fake provider 能跑通完整工作流。
- 真实 LLM 测试默认 ignored，只从环境变量读取凭据。
- LLM 不通过自由文本或强制工具调用控制 workflow 状态机。

## 11. 阶段 7：业务 HTTP API

最小 API：

```text
GET    /health
GET    /ready
POST   /v1/workspaces
GET    /v1/workspaces
DELETE /v1/workspaces/{id}
POST   /v1/workspaces/{id}/prompts
GET    /v1/requests/{id}
GET    /v1/requests/{id}/events
POST   /v1/requests/{id}/cancel
GET    /v1/logs/stream
```

工作内容：

- 只有 Workspace/Workflow capability 就绪时才注册 prompt 路由。
- 统一认证、幂等键、背压、body limit 和 JSON 错误。
- 使用 SSE 传输业务事件，明确 lag、重连和事件游标语义。
- loopback 默认允许无认证；非 loopback 强制 token。

验收门槛：

- HTTP → ECS → persistence → workflow → result stream 全链路通过。
- 已接受请求不会因进程重启静默消失。
- malformed JSON、鉴权失败、队列满和关闭状态有稳定响应。

## 12. 阶段 8：完整 CLI

目标命令：

```text
margatroid status
margatroid up -f compose.toml
margatroid ps
margatroid stop <workspace>
margatroid prompt <workspace> <text>
margatroid inspect <request-id>
margatroid watch <request-id>
margatroid cancel <request-id>
margatroid logs --follow
```

CLI 只负责参数、HTTP、展示和退出码，不包含业务状态机。

验收门槛：

- 在临时目录启动真实 daemon，黑盒执行完整用户路径。
- 人类可读输出和机器可读 JSON 输出均有稳定契约。
- 网络失败、认证失败和业务失败使用不同退出码。

## 13. 阶段 9：生产硬化

工作内容：

- 完成威胁模型、token 比较、CORS、路径穿越和符号链接审计。
- Sandbox 采用默认拒绝策略，并设置资源限制。
- 覆盖队列满、慢客户端、LLM 超时、磁盘满、数据库锁和崩溃恢复。
- 完善结构化日志、request correlation、health 和 readiness。
- 建立 Linux x86_64 和 macOS arm64 CI。
- 清除 warning，并加入依赖、许可证和安全审计。

验收门槛：

- 全 workspace `fmt`、Clippy `-D warnings`、测试和依赖审计通过。
- 连续运行、压力和故障注入测试无静默丢失、死锁或无限增长。
- 源码、日志、错误和 Debug 输出不泄漏 secret。

## 14. 阶段 10：发布工程

Margatroid 产品：

- 补齐 LICENSE、SECURITY、CHANGELOG 和版本策略。
- 发布 `margatroidd` 与 `margatroid` 二进制和 checksum。
- 提供示例配置、示例 compose、systemd 和 launchd 文档。
- 在全新机器验证安装到首个 workflow 结果。

mecs crates：

- 统一 crates.io 名称，例如 `mecs-core`、`mecs-runtime`、`mecs-http`。
- 补齐 description、repository、license、readme、rust-version 和 categories。
- 去除不可发布的 path-only 依赖，收敛 feature 和公开 API。
- 按依赖顺序执行 package dry-run、publish 和 docs.rs 验证。

基础设施公开 API 在被完整产品链路实际使用前不冻结；先 dogfood，再发布 mecs 0.1。

## 15. v0.1 最终发布门槛

- 全新用户能按文档在 15 分钟内跑通首个 workflow。
- daemon 重启不丢失已接受任务。
- CLI 可以完成 workspace 和任务全生命周期。
- 正式构建不依赖 legacy，无源码密钥，无默认公网暴露。
- 支持 Linux x86_64 和 macOS arm64。
- 完整测试、Clippy、依赖审计和真实 LLM smoke test 通过。
- 二进制、checksum、许可证、升级和卸载说明齐全。
- Web UI、MCP、bridge、分布式和动态 Plugin 明确留到 v0.2+。

## 16. 执行规则

- 每个阶段先稳定 public API，再实现内部逻辑。
- 每个阶段必须包含测试、README、设计文档更新和独立 commit。
- 不为追求形式一致扩大 core；领域复杂度留在业务 Plugin。
- 不从 legacy 复制完整架构，只迁移经过重新划分职责的局部能力。
- 不使用真实凭据作为默认测试条件。
- 当前阶段未通过验收前，不并行启动下一个关键路径阶段。
