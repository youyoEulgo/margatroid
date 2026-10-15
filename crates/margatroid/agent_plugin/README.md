# AgentPlugin

AgentPlugin负责Agent实例的启动、停止、异常关联与生命周期；共享Agent组件保存身份、Workspace关联、配置快照、Lua通知句柄、execution作用域、MCL队列、turn、导入绑定及数据库/历史句柄。Block在真实SQLite中，Agent不保存Block权威内存内容。消息历史只由显式MCL `history_append`追加，推理与工具仅返回结果，不自动保存消息。

## 已批准目标：轮次中止（待实现）

以下是批准的目标设计，**不是已实现功能**。详细目标与现有基线见[设计](<DESIGN.md>)；当前代码的运行期`Running`及无条件轮次生成行为尚待迁移。

- 目标状态为`Creating / Idle / Working / Stopping`。启动成功进入`Idle`、`turn=None`。`start_turn`仅允许Idle，原子创建turn并置Working，不覆盖已有轮次。
- abort是独立控制事件，不进入MCL FIFO。控制System先于MCL继续动作及Inference/Tool受理执行；Working收到abort立即清空`agent.turn`，仍保持Working，直到Driver沿正常消息链调用`finish_turn`才进入Idle。Working且turn=None重复abort幂等pending。
- 协议目标`agent.turn.abort {id, message:{workspace, agent?, turn_id}}`要求非空turn_id；服务器内部核对实际Agent实例，不接受用户Entity。`agent.turn.abort_result`按id关联，Ok只表示控制受理，不代表清理完成或Idle；同一旧轮次重试使用保留取消关联，不建立平行旧pending权威。
- 五种观察点（abort前Working且turn存在）均须覆盖：inference未受理、inference已受理、tool_call未受理、tool_call已受理、Driver消息处理/下个effect之间的间隙。未受理请求取消路径claim一次性reply恰好一次；已受理请求取消token并等待真实清理，之后返回正常形状取消结果；Driver间隙由下一effect实际动作的turn门覆盖。Working且turn=None的重复abort另按幂等pending处理。finish_turn须对应当前Driver结果链且本轮清理已完成，Idle重复完成幂等，不得以迟到旧完成覆盖新的Working轮次。
- `inference/tool_call`在异步准备后的实际effect动作处要求turn。无turn返回Ok取消Assistant/工具结果，而非通用取消错误；不新增`get_turn`，Driver不检查turn。
- Assistant消息携带`stop_reason=Cancelled`（序列化`cancelled`），取消正文为`Turn aborted.`，`tool_calls=[]`、`usage=None`；工具取消保留调用ID与顺序，`failed=true`、正文`Tool call aborted.`。消息历史/Block/投影保留stop_reason。
- Driver读取entry第一条并移除同一条，显式历史保留原值。Inject仅递归展开容器并追加非容器上下文（嵌套取消Assistant也跳过），不执行子消息控制分支；User追加上下文、start_turn、推理；取消Assistant仅入历史，正常Assistant入上下文，无调用Assistant是唯一finish_turn路径，否则发工具批次；全部工具结果入上下文，批次齐后即使turn=None仍推理，取得取消Assistant收尾。Error不得直接finish或伪造取消消息。
- 前端仅Idle允许发送、Working允许abort；服务端用户消息受理也必须验证Idle，不能只依赖按钮。清理失败仍返回错误并保留未清理活动，不转Idle。

轮次abort不是停止Agent，不关闭Driver或根execution作用域。Creating/Stopping、StopAgent根取消、全部活动真实完成、MCL回执drained与失败保留宿主的原清理语义不变。本次仅文档修订，不声明代码、协议或前端已经实现目标。
