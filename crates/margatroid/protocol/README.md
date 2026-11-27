# Protocol

Protocol 定义纯 JSON 信封、生命周期请求、分类状态、记录页与消息同步，只依赖标准库、serde 等通用库，不依赖业务 core、plugin、address 或 World。实体所属类型、任务受理、调用权限和真实执行结果由 DTO 与领域实现核对。

当前源码实现52种消息：21种客户端请求、31种服务端消息，其中21种一次性响应、10种主动同步。此说明描述源码契约，不表示运行中的 daemon、客户端或独立 API 仓库中的机器 Schema 已更新。字段说明见[WebSocket API](../../../../doc/api/WS_API.md)，执行链见[API Entry](../../../../doc/api/API_ENTRY.md)。

## 网络消息

请求信封为 `{type,id,message}`，message 必须显式为对象。响应和同步的数据位于顶层。`id` 非空、无控制字符且最多512个 UTF-8 字节；每次响应保留原请求 ID，sync 不含 ID。未知消息、未知字段和不属于当前变体的字段明确拒绝。

```text
connection.register / connection.register.result
model.config.req / model.config.res / model.config.sync
model.config.update / model.config.update.result
workspace.req / workspace.res / workspace.sync
workspace.create / workspace.create.result
workspace.delete / workspace.delete.result
workspace.rename / workspace.rename.result
agent.rename / agent.rename.result
resource.req / resource.res
agent.req / agent.res / agent.sync
agent.start / agent.start.result
agent.stop / agent.stop.result
agent.archive / agent.archive.result
agent.ui.req / agent.ui.res / agent.ui.sync
agent.turn.req / agent.turn.res / agent.turn.sync
agent.turn.abort / agent.turn.abort.result
agent.task.req / agent.task.res / agent.task.sync
agent.history.req / agent.history.res
agent.log.req / agent.log.res / agent.log.sync
agent.mcl.req / agent.mcl.res / agent.mcl.sync
agent.message.chunk.sync / agent.message.full.sync
mcl.execute / mcl.execute.result
```

旧 Workspace start/stop/restart/update 及其结果不再接受；workspace.rename现在只表示别名修改，不提供旧节点或目录重命名语义。也没有 agent.restart、统一 state 包或独立 finished 消息的兼容入口。req/res 为纯查询，sync 由实际变化驱动；两者都不执行目录扫描、安装监控或推进工具输出游标。sync 与命令响应按各自完成时机发送，不能互相替代。

## 节点与生命周期

运行实体地址为绝对节点路径，例如 `/demo`、`/demo/coder`、`/demo/coder/reviewer`。协议拒绝相对地址、空段、尾部斜杠、点段、反斜杠、冒号和控制字符，保留大小写。协议只校验路径形状，Workspace/Agent 类型由 World 中的组件判定。文件资源继续采用 `type:scope/name:tag`，tag 必须显式非空；这些资源字符串不是运行实体地址。

`WorkspaceCreateDto` 携带 address、alias、绝对 project_root；创建地址只允许根下单段节点。`WorkspaceInfoDto` 提供 address、alias、project_root、status、agents，不再提供 YAML 定义或扫描项 path。Workspace 状态为 running/deleting/error；agents 是数据库确认提交的登记记录，类别为 normal/temporary/archived，不表示实体运行状态。

`WorkspaceAgentInfoDto` 保留 id、parent_node、node_name、alias、category、tool_path、tool_arguments、previous_node_name、previous_alias。nullable 字段必须出现；普通节点名非空且在同一 Workspace 内唯一，临时及归档记录的 node_name 为 null。tool_arguments 是上次实际进入登记提交的完整 Driver 调用记录，保留自定义内容，不被协议递归当成 AgentMessage 解释。Existing 启动将以当前登记的node_name/alias重建同名参数，不把这份历史记录当成别名权威来源。

`WorkspaceRenameDto`使用address和非空alias；`AgentRenameDto`使用workspace、持久agent_id和必填nullable alias，null表示清空。alias必须非空、非全白、无控制字符且最多512字节；不裁剪合法首尾空白。agent_id保留opaque字符串，非空、非全白、无控制且最多512字节，不当作路径解析。严格拒绝Agent旧address定位或省略alias，成功结果只含type/id/success，别名由workspace.sync交付；不改变节点、目录、Driver参数或归档previous_alias。

`AgentStartInputDto` 使用 mode 区分两个互斥变体：

```text
existing { workspace, node_name }
new      { workspace, tool_path, arguments }
```

existing 的 node_name 是 Workspace 内普通节点名；new 的节点名、展示别名及工作参数位于 arguments 内，不重复放在顶层。tool_path 为绝对文件路径。启动网络请求只发起 Driver 工具调用，工具成功返回不证明 Agent ready 或任务结束。创建文件、连接数据库、登记及实体装配由 Driver 的创建注入方法和领域事件完成，网络不传数据库句柄。

agent.stop 按目标 Agent 地址定位创建任务，等待任务及全部绑定 Agent 清理；登记和数据保留。agent.archive 使用 Workspace 和节点名定位记录，等待关闭、目录移动和数据库归档完成。错误不承诺所有副作用已回滚，客户端不能因连接超时自动重发写操作。

Agent 运行状态为 starting/running/closing，与登记类别独立。Agent 对象的 creator 必有且非 null，包含真实父节点地址与任务 ID；不暴露 Entity、运行 instance、锁或连接。turn 单独同步，工具、沙箱和模型公共信息保留完整记录；模型端点和凭据不进入 Agent 状态。

## 消息与记录

内部 AgentMessage 保留 system、user、assistant、compact、tool_call、inject、error。compact 与 assistant 同结构且保留 type；reasoning、正文各一份，不加入厂商 replay、签名、加密内容或 images 别名。媒体使用 data 数组，项为 media_type 与标准 base64 data。历史消息保留真实正整数 sequence，不能用 chunk 序号或 Block 数组位置替代。

历史、MCL 与日志分别查询。消息与 MCL 共用历史序号，日志使用独立序号。分页边界 before_sequence/after_sequence 为排他界，正 limit，双界要求 after<before；输出升序，成功必有两个 nullable 游标，失败省略记录及游标。查询不消费数据；日志和 MCL sync 只交付新增已提交记录，不发送空增量。

Task 查询与同步提供完整活跃集合和各自保留输出窗口，包含已 return 尚未清理的后台任务。returned 不表示任务结束，真实清理后移除，不提供 completed 缓存，也不修改脚本读取游标。

chunk 提供同一 generation 截至 sequence 的完整累计 content/reasoning，两者必有且为字符串、至少一侧非空。客户端替换文本，忽略同 generation 迟到或重复序号。turn 必有且可空，自由调用为 null。full 一次性交付生成终态，history_sequence=null，不证明落库；非生成持久消息的 full 使用真实 history_sequence 并与 message.sequence 一致。保存已生成 assistant/error 不产生第二条终态 full。

MCL 成功的非 Unit 返回 data.kind/value：turn、messages、config、import、inference、block_snapshots。Unit 省略 data；MCL Error 映射失败响应。推理领域错误可作为成功交付的 inference 值，其 message 为 error、stop_reason=error，不代表模型成功。显式 turn 必须匹配当前 running Agent，自由调用不继承当前回合；中止只影响明确关联的执行。

## 配置、UI 与校验

model.config 读取或同步原始 models.toml，更新完整校验后原子替换同一文件，不接收客户端路径。原文可能包含凭据，Agent.model 只提供安全模型字段。原文最多1MiB；文件无效时可交付原文供修复，推理不回退旧路由。

agent.ui 返回创建方法指定目录的完整资源集合，包含 SHA-256 revision、index.html 入口和排序唯一的 files；文件内容统一 base64。目录缺失可为 null；读取失败按对应 error 分支表示。nullable ui/error 必须按各消息规则出现。

网络请求、响应和同步信封不设置整体字节上限；WebSocket帧和消息入口也不叠加框架默认大小上限。错误 message 最多512字节。媒体保留 PNG/JPEG/GIF/WebP、每消息最多8张、每张解码5MiB、合计20MiB的现有校验。ServerMessage 的编码与解码共同核对成功/失败互斥、必需载荷、nullable 字段、记录顺序及序号关联；不根据无效目标修改领域状态。

域内测试覆盖18种请求、28种出站、旧接口拒绝、互斥启动变体、登记类别、节点路径、必需 nullable 字段、累计文本、分页及显式回合关联。
