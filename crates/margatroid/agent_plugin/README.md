# AgentPlugin

AgentPlugin负责Agent实例的启动、停止、异常关联与生命周期；共享Agent组件保存身份、Workspace关联、配置快照、Lua通知句柄、execution作用域、MCL队列、turn、导入绑定及数据库/历史句柄。Block在真实SQLite中，Agent不保存Block权威内存内容。消息历史只由显式MCL `history_append`追加，推理与工具仅返回结果，不自动保存消息。

## 集中工作区成员契约（目标与实现边界）

工作区定义统一持久化于主目录，数据库路径由工作区存储能力推导为 `~/.margatroid/workspaces/<workspace>/agents/<agent>/state.sqlite`。AgentPlugin 继续按类型化启动材料打开现有单库并管理真实执行清理，不扫描或写 workspace.yaml。成员加入/移除由 Workspace 生命周期协调持久化；普通 StopAgent 不等于从工作区定义移除，也不删除数据库。移除成员必须等待真实 StopAgent 清理完成，持久化失败仍需保留定义并报告实际已停止事实。manager 已从工作区契约删除，面向 Agent 的协议请求必须明确携带成员 ID。


## 名称、驱动与重命名边界

目标定义位于 `~/.margatroid/workspaces/<workspace>/workspace.yaml`，状态位于同目录 `agents/<agent>/state.sqlite`；不使用项目级 `.margatroid` 或manager。Workspace/成员名称派生ResourceID，名称不变时稳定；重命名是显式迁移，不另引入稳定身份模型。用户及sidebar选择的驱动是现有AgentDriver包，New session增加成员，不是另一种Driver；可选ID应来自驱动目录的发现结果/DriverCatalog，不仅来自已挂载的运行中集合。

Workspace重命名只在停止且所有相关DB句柄关闭后受理；运行中成员重命名先关闭新受理、drain/StopAgent并等待真实清理，然后关闭所有句柄。随后由Workspace编排共享类型化迁移能力：在SQLite备份副本迁移已知Block namespace、重建当前history.query身份/句柄投影元数据，保持原始历史正文、MCL文本/参数、来源、顺序、时间与工具事实不变，禁止盲目字符串替换。内部 `.operations/<operation>/` 日志、staging及备份排除于工作区catalog，阶段fsync；目录、manifest和SQLite各自提交，失败保留可恢复数据并报告部分完成，重启先恢复/标记不可用再开放受理。该迁移与恢复器已实现并有 scoped 与真实存储集成验证；机械SQLite细节见[Memory设计](<../memory_plugin/DESIGN.md>)。

恢复已完成；存储迁移与领域路由已接通。下述行为按当前实现描述，验证范围不扩张为所有故障的保证。

## 轮次中止

Agent生命周期层已实现`Idle / Working`及独立`AbortAgentTurn`控制事件。控制系统注册在`RuntimePlugin::PRE_UPDATE`，必须先于MCL与领域受理；daemon 将 InferencePlugin 显式配置于 UPDATE，组合根仍须核对实际系统顺序，不能只凭插件默认计划名保证优先；自定义调度必须保留这个先后关系。只保留当前实例最近一次已中止turn的关联，重复请求仅在Working收口期间成功。详细契约与跨插件行为见[设计](<DESIGN.md>)。

- 目标状态为`Creating / Idle / Working / Stopping`。启动成功进入`Idle`、`turn=None`。`start_turn`仅允许Idle，原子创建turn并置Working，不覆盖已有轮次。
- abort是独立控制事件，不进入MCL FIFO。控制System先于MCL继续动作及Inference/Tool受理执行；Working收到abort立即清空`agent.turn`，仍保持Working，直到Driver沿正常消息链调用`finish_turn`才进入Idle。Working且turn=None重复abort幂等pending。
- 协议目标`agent.turn.abort {id, message:{workspace, agent, turn_id}}`要求非空turn_id；服务器内部核对实际Agent实例，不接受用户Entity。`agent.turn.abort_result`按id关联，Ok只表示控制受理，不代表清理完成或Idle；同一旧轮次重试使用保留取消关联，不建立平行旧pending权威。
- 五种观察点（abort前Working且turn存在）均须覆盖：inference未受理、inference已受理、tool_call未受理、tool_call已受理、Driver消息处理/下个effect之间的间隙。未受理请求取消路径claim一次性reply恰好一次；已受理请求取消token并等待真实清理，之后返回正常形状取消结果；Driver间隙由下一effect实际动作的turn门覆盖。Working且turn=None的重复abort另按幂等pending处理。finish_turn须对应当前Driver结果链且本轮清理已完成，Idle重复完成幂等，不得以迟到旧完成覆盖新的Working轮次。
- `inference/tool_call`在异步准备后的实际effect动作处要求turn。无turn返回Ok取消Assistant/工具结果，而非通用取消错误；不新增`get_turn`，Driver不检查turn。
- Assistant消息携带`stop_reason=Cancelled`（序列化`cancelled`），保留已有正文和思考，无输出保持空值，`tool_calls=[]`、`usage=None`；工具取消保留调用ID与顺序，`failed=true`、正文`Tool call aborted.`。消息历史/Block/投影保留stop_reason。
- Driver读取entry第一条并移除同一条，显式历史保留原值。Inject仅递归展开容器并追加非容器上下文（嵌套取消Assistant也跳过），不执行子消息控制分支；User追加上下文、start_turn、推理；取消Assistant仅入历史，正常Assistant入上下文，无调用Assistant是唯一finish_turn路径，否则发工具批次；全部工具结果入上下文，批次齐后即使turn=None仍推理，取得取消Assistant收尾。Error不得直接finish或伪造取消消息。
- 前端仅Idle允许发送、Working允许abort；服务端用户消息受理也必须验证Idle，不能只依赖按钮。清理失败仍返回错误并保留未清理活动，不转Idle。

轮次abort不是停止Agent，不关闭Driver或根execution作用域。Creating/Stopping、StopAgent根取消、全部活动真实完成、MCL回执drained与失败保留宿主的原清理语义不变。协议、Driver与前端支持由各自插件提供；本插件的成功回执仅证明中止控制已受理。
