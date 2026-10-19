# DtoPlugin

## 当前状态

当前 catalog、注册启动、成员增删、工作区创建/改名/删除、成员改名、Driver 发现及后端目录浏览均已实现并完成恢复验证。验证仅覆盖已执行的 scoped、真实存储集成和隔离数据浏览器场景，不构成所有故障或环境的普遍保证。

## 职责与目标路由

DtoPlugin 处理 WebSocket 信封、真实 Client 身份、协议转换、领域事件和回执关联，不在 ECS 主线程读取 YAML、SQLite、目录或执行迁移。Protocol 是 wire 字段权威，领域事件/能力由共享 types 定义。

| 请求 | 接收/投影 |
| --- | --- |
| connection.register | 注册，等待真实 Client 建立后回执 |
| workspace.catalog.query / refresh | 共享目录快照/异步刷新；workspace.catalog |
| workspace.start | ID+expected_revision → StartRegisteredWorkspace；started/start_failed |
| workspace.stop | 受检实例停止；stopped/stop_failed |
| workspace.member.add / remove | 持久成员增删；workspace.member.result |
| workspace.create / rename / delete / member.rename | 已实现的持久变更；workspace.member.result |
| workspace.paths.query | 后端项目目录选择页；workspace.paths |
| driver.catalog.query / refresh | 有效安装包清单；driver.catalog |
| mcl.command | 显式 Agent 单条 MCL，source 绑定真实 Client |
| agent.turn.abort | 完整实例及预期 turn 的独立控制请求 |
| state.query / history.query | 元数据/句柄快照+blocking 历史读取 |

新建工作区只接收名称和已存在绝对项目路径；新增成员只接收名称、所选 canonical driver ID 和可选 home_root。路径/数据库/身份由服务端推导，空 Workspace 合法，无 manager。MCL/history/abort 的 Agent ID 必填，缺失/null 明确拒绝。

## 响应与一致性

目录、变更、start/stop、abort、路径和 Driver 请求使用内部唯一 operation_id，网络 id 在响应时还原。按操作种类匹配结果，再检查 Client 完整 Entity generation/连接，防止不同 Client 同 id 串回执。重复在途请求拒绝；断连清理映射但不撤销已经受理的领域操作，迟到结果不发给新连接。

workspace.member.result 统一表达定义变更：成功有 entry、无 error；失败 entry=null 且有 error；**删除工作区成功 entry=null、无 error**，不能因为条目已不存在而报错。持久成功后成员启动失败保留已保存定义并在 runtime 错误/member_failures/logs 显示。Stop 成功但保存失败要明确已停止/仍配置，不伪造原子回滚。

状态按 `catalog/workspaces/agents/histories/drivers` 分开投影。全部注册项（含 stopped/invalid）来自 catalog，runtime 只表达真实实例，历史/文件存在不证明运行。无效定义不能隐藏 live 实例。内容/接收者变化才主动同步；目录和 Driver 变化也必须参与去重判断。

目录选择通过 daemon 注入的 ProjectDirectoryBrowser 在 blocking worker 读目录；默认 HOME，每页最多 200，规范路径、权限、失效 cursor 见 Protocol。不能在 DTO 同步扫描，不能把浏览器文件名当主机路径。Driver 由 loader 验证安装包后发布，含尚未挂载包，只输出规范 ID/诊断；真正加载再次校验。

## 输入、日志及历史保护

入站总 32 MiB；超限关闭 1009，无法关联的非法结构关闭 1007，有合法 id 返回 request.failed。没有旧 1 MiB 静默丢弃。诊断不回显 base64、凭据或原始私有载荷。

outbound 仅 logs/backend_state 两项。logs 控制主动日志分发，backend_state 控制主动 state.sync；请求回执直接返回发起连接。TracingStream 是临时有界广播，慢订阅可丢日志，没有离线补发承诺；日志发送失败不递归写日志。

历史读取在 blocking worker，旧 generation 结果不绑定新实例。正文 content 与 payload 元数据分离，images/replay/usage/stop_reason/工具 ID 保留；created_at_ms 使用真实持久值，旧 0 表示未知。消息历史只由显式 history_append 决定，不因推理或工具完成自动生成。

中止受理 Ok 不证明清理完成：清 turn 仍 Working，真正 stop/abort+await 和 Driver finish_turn 后才 Idle。取消 Assistant 保留已有内容、无输出为空，允许历史且排除模型上下文；前端普通渲染。Working 手工 INJECT 递归 User 拒绝，不把非用户资源数组误当 User。根关闭、timeout、receiver 断开和 CleanupFailed 仍有独立原因。

## 验证与范围

当前 DTO/Protocol scoped 验证合计 34 passed，覆盖恢复的 catalog、registered-start、成员与 CRUD/discovery 路由、精确网络 ID、generation 和错误关联。领域存储迁移与真实浏览器交互另有独立验证，不能由 DTO 测试替代。
