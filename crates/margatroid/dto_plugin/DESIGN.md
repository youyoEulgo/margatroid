# 伪代码格式
```text
模块：使用一级标题，只写当前设计涉及的部分

类型：使用二级标题，按私有、crate公开和公开分组
TypeName：中文类型名，可见性类型--类型说明
    field_name: RustType--中文字段名，字段说明
    method_name<Generic>(self, parameter: ParameterType) -> ReturnType
        中文方法名：可见性方法，解释参数和用途
        约束：使用标准Rust where约束；单个约束写在同一行
        行为：展开完整逻辑
    impl TraitName for TypeName
        TraitName：可见性trait实现
        trait_method_name(&self, parameter: ParameterType) -> ReturnType
            中文方法名：解释参数和用途
            行为：标准库trait的简单行为用一句话说明

函数：使用二级标题，按私有和公开分组，只放置不属于某个类型的操作
function_name<Generic>(parameter: ParameterType) -> ReturnType
    中文函数名：可见性函数，解释参数和用途
    约束：使用标准Rust where约束；单个约束写在同一行
    行为：展开完整逻辑

逻辑：使用二级标题，按执行顺序描述对象之间的调用关系
注释：字段注释使用--，类型、方法和函数的说明直接写在标题中
属性：不写Rust Attribute，实现时自行判断
边界：对外使用泛型和具体类型，内部使用类型擦除
```

# 板块格式

每个 crate 的 DESIGN.md 在伪代码格式之后使用六个一级标题，按 `lib`、`system`、`handler`、`events`、`types`、`error` 顺序组织。每个标题对应 `src/` 下的同名 Rust 文件：

```text
# lib        src/lib.rs        图书馆组件与 Plugin
# system     src/system.rs     System 函数
# handler    src/handler.rs    处理函数
# events     src/events.rs     事件类型
# types      src/types.rs      其余类型
# error      src/error.rs      Error 与公开错误分类
```

# 当前实现与旧设计归档

当前实现以README及protocol当前wire为准。下文六板块保留为迁移前设计档案（已实现的中止契约节除外），不再要求恢复其中旧Mailbox、AgentFailure、资源visibility、Memory和oneshot pending轮询接口。

当前入站为connection.register、workspace.start/stop、agent.turn.abort、mcl.command、state.query、history.query；source绑定共享Client.resource_id。共享WebSocketMessageSend为target+serde_json::Value，事件定义在types::dto_events，插件仅re-export。MCL结果按id/source/target关联并核对完整Client Entity；Workspace结果使用内部唯一operation_id还原网络id。断连清理MCL/start/stop/abort/snapshot网络pending，不取消已提交领域操作，迟到结果不重建网络关联。

状态来自共享Agent.state/turn/sandbox/mcl和history.list；快照及接收连接集合未变化时不发送，避免event-driven自循环。投影sequence为list顺序1-based，created_at_ms保留HistoryRecord创建时间，旧记录0表示未知。旧实时Memory、消息广播与AgentTokenUsage聚合不保留。

## 用户轮次中止契约（已实现）

独立控制请求转换为共享 AbortAgentTurn；网络层仅保存发起连接与完整领域关联，不恢复旧邮箱或执行 pending 轮询。无 `get_turn`。

- 请求：`{ "type":"agent.turn.abort", "id":"abort-1", "message":{ "workspace":{id,name,project_root}, "agent":null, "turn_id":"turn-1" } }`。`id` 为网络请求关联，`turn_id` 为必填非空的预期活动轮次；agent 为 null 时选择 manager，其余身份规则与 MCL 相同。
- 回执：`agent.turn.abort_result { id, workspace, agent, turn_id, result }`；result 使用 `{"Ok":null}` / `{"Err":{kind,message}}`。Ok 仅确认中止已受理或该关联轮次已中止，不证明执行 cleanup 已结束；失败保留请求 id。仅回复发起连接，source 来自真实注册 Client。
- 入口受检解析 Workspace 与当前成员的完整实例，实际处理再次核对 turn_id；旧/不匹配轮次不得取消新轮次。对同一已中止轮次（含 Working 且 turn=None）重复请求幂等成功，仅保留必要已中止关联，不恢复全局 pending 表；未识别轮次报关联错误。
- 状态为 `creating/idle/working/stopping`。中止立即清空 turn，真实清理期间保持 Working，正常 finish 收口才 Idle；状态同步同时发布 turn 与 status。前端据 status 显示运行态、据 turn 关联中止，不能把回执或 turn=None 当作 Idle。
- AgentPlugin 将 abort_agent_turn_system 固定安装到 RuntimePlugin::PRE_UPDATE，先于 UPDATE 中的 MCL 实际执行及 inference/tool admission。共享事件允许各消费者独立观察，不靠 take reply 独占中止通知；执行前再次核对请求轮次，turn=None 或已中止旧轮次不启动推理/工具而返回正常取消结果。
- Assistant 新增 stop_reason，覆盖全部推理原因，新增 `StopReason::Cancelled` 序列化 `cancelled`。兼容保留 `InferenceResponse.stop_reason`，单一权威映射生成，两处始终相同。用户轮次中止且真实清理成功返回 Ok assistant：`content="Turn aborted."`、`reasoning=None`、`tool_calls=[]`、`usage=None`、`stop_reason=Cancelled`。
- 取消工具且清理成功返回 Ok ToolCall：`failed=true`、`content="Tool call aborted."`，保留调用 ID、名称及已知 resource_id。批次保留已完成真实结果，其余每个调用（包括未启动项）各有一个原序结果；不虚构部分执行成功，不撤销已发生副作用。
- `CleanupFailed` 仍为 Err，保留活动，不伪造取消或 Idle。用户轮次中止不同于根 Agent shutdown、owner abort、timeout、receiver closed，不统一转换这些原因。
- 历史只经显式 history_append 保存；Assistant stop_reason 存入 payload 元数据而非 content。cancelled assistant 可入历史，不得进入 infer 上下文（包括 Inject 嵌套）。

# lib

以下为归档设计：lib 放 Plugin、安装标记 Resource 和公开 re-export。

## 类型

公开：
```text
WebSocketMessageTarget：发送目标，公开枚举--由config_plugin重新导出
    Broadcast--全部连接
    Type(String)--指定connection_type的连接
    Name(String)--指定唯一连接名称

DtoPlugin：WebSocket DTO转换插件，公开结构体--注册WebSocket路由并在协议DTO与领域事件之间转换
    websocket_path: String--WebSocket路由路径，私有
    schedule: String--dto_route_system所属Runtime schedule，私有
    new() -> Self
        构造插件：公开关联函数，使用默认路径/ws和RuntimePlugin::UPDATE
    with_websocket_path(mut self, path: impl Into<String>) -> Self
        设置路径：公开构建方法
    with_schedule(mut self, schedule: impl Into<String>) -> Self
        设置Schedule：公开构建方法
    impl Default for DtoPlugin
        Default：公开trait实现，调用new
    impl Plugin for DtoPlugin
        Plugin：公开trait实现
        build(self, app: &mut App)
            安装插件：公开trait方法
            行为：
                重复安装时panic
                Schedule不存在时panic
                ServerPlugin未安装WebSocketConnections时panic
                读取MargatroidConfig和TracingStream，启动forward_logs异步日志转发
                插入DtoPluginInstalled、BackendStateReportCache和PendingMclCommandResponses
                注册WebSocket事件路由，并在schedule挂载collect_external_events_system和dto_route_system
```

私有：
```text
DtoPluginInstalled：DtoPlugin安装标记，私有单元Resource--阻止同一App重复安装
    impl Resource for DtoPluginInstalled
```

# system

system 放 System 函数。System 只读取本帧事件并克隆，然后调用 handler。

## 函数

crate公开：
```text
dto_route_system(world: &mut World)
    路由WebSocket API：crate公开System
    处理事件：WebSocketMessageReceived、WebSocketMessageSend
    行为：
        克隆本帧全部WebSocketMessageReceived，逐个调用handle_inbound_message
        调用handle_pending_mcl_responses处理已完成的MCL命令
        克隆本帧全部WebSocketMessageSend，调用handle_outbound_messages

collect_external_events_system(world: &mut World)
    收集外部事件：crate公开System
    行为：调用handle_collect_external_events
```

# handler

handler 放处理函数。入站解包、出站序列化、外部事件投影和日志转发都在此展开。

## 函数

crate公开：
```text
handle_inbound_message(world: &mut World, connection_id: WebSocketConnectionId, message: WebSocketMessage)
    处理入站消息：crate公开函数
    行为：
        要求消息为Text
        反序列化统一{type,id,message}信封为ClientMessage
        注册检查：connection.register以外的请求要求连接已注册，未注册时写warn日志并丢弃
        按type调用对应DTO::into_domain并发送领域事件
        MCL命令先用mcl_source算出客户端来源标识并随转换上下文传入
        MCL命令转换成功后把响应接收器写入PendingMclCommandResponses
        转换失败时写warn日志并丢弃当前请求

注册检查的边界：只看处理该请求这一帧的状态——连接此时已注册就服务，否则丢弃；不做延后、不重试。
            回执在Client实体创建之后发出，等待回执再发后续请求的客户端必然满足该条件；
            不等待回执的客户端落在契约之外，其请求服务与否取决于注册与请求两帧是否落在同一帧，daemon不作保证

handle_pending_mcl_responses(world: &mut World)
    处理MCL响应：crate公开函数
    行为：
        读取WebSocketConnections
        逐个尝试接收PendingMclCommandResponses
        已完成的响应序列化为MclCommandResult并发送到原连接
        Empty保留，Disconnected移除

handle_outbound_messages(world: &mut World, outgoing: Vec<WebSocketMessageSend>)
    处理出站消息：crate公开函数
    行为：
        读取WebSocketConnections
        逐条序列化ServerMessage为Text
        按Broadcast、Type或Name解析发送器集合
        调用WebSocketMessageSender::try_send；Log消息发送失败不写warn

handle_collect_external_events(world: &mut World)
    收集外部事件：crate公开函数
    行为：
        读取MargatroidConfig目标
        依次调用report_server_events、report_client_registrations、report_workspace_events、report_workspace_stop_events、report_agent_messages、report_agent_failures和report_backend_state

forward_logs(stream: TracingStream, events: RuntimeEventSender, targets: Vec<WebSocketMessageTarget>)
    转发结构化日志：crate公开异步函数
    行为：
        订阅TracingStream
        把TracingRecord转换为LogRecordDto并按logs目标发送WebSocketMessageSend
        Lagged时继续，Closed时结束
```

私有：
```text
mcl_source(world: &World, connection_id: WebSocketConnectionId) -> String
    计算MCL来源标识：私有函数，从WebSocketConnections读取该连接的类型与名称
    行为：两者都存在时调用client_plugin::client_source得到client:<type>/<name>:<connection_id>；
          任一缺失（连接未注册或注册未完成）时得到unregistered:<connection_id>

report_server_events(world: &World)
    报告Server事件：私有函数，将Server启停、WebSocket连接、断开和协议失败投影为结构化日志

report_client_registrations(world: &World)
    报告客户端注册：私有函数，把ClientRegistrationResult投影为ServerMessage
    行为：成功时构造ConnectionRegistered（含ClientInfoDto），失败时构造ConnectionRegisterFailed；两者都按result.connection_id直达该连接
    边界：回执属于请求-响应，与mcl.command_result同样绕过WebSocketMessageSend的目标分类，直接向发起请求的连接发送

send_to_connection(world: &World, connection_id: WebSocketConnectionId, message: &ServerMessage)
    按连接发送：私有函数，从WebSocketConnections取WebSocketSender并try_send序列化文本；连接不存在或序列化失败时静默返回

report_workspace_events(world: &World, targets: &[WebSocketMessageTarget])
    报告Workspace启动：私有函数，成功时发送WorkspaceStarted，失败时写error日志并发送WorkspaceStartFailed

report_workspace_stop_events(world: &World, targets: &[WebSocketMessageTarget])
    报告Workspace停止：私有函数，发送WorkspaceStopped或WorkspaceStopFailed并记录日志

report_agent_messages(world: &World, targets: &[WebSocketMessageTarget])
    报告Agent消息：私有函数，将外部可见User和Assistant转换为AgentMessage协议事件；跳过System、Tool和Error；Error只通过历史状态同步渲染

report_agent_failures(world: &World, targets: &[WebSocketMessageTarget])
    报告Agent失败：私有函数，将AgentFailure转换为协议事件并记录warn

report_backend_state(world: &mut World, targets: &[WebSocketMessageTarget])
    报告后端状态：私有函数
    行为：
        构造BackendStateDto
        转换失败时相同错误只记录一次
        解析backend_state接收连接并排序去重
        仅在首次运行、快照变化或接收连接集合变化时发送StateSync

send_to_targets(world: &World, targets: &[WebSocketMessageTarget], message: ServerMessage)
    按目标发事件：私有函数，为每个目标克隆ServerMessage并发送一个WebSocketMessageSend

target_recipients(connections: &WebSocketConnections, targets: &[WebSocketMessageTarget]) -> Vec<u64>
    解析接收连接：私有函数，按目标查询发送器并返回连接ID；排序和去重由调用方完成
```

# events

events 放事件类型。

## 类型

公开：
```text
WebSocketMessageSend：发送事件，公开结构体--承载ServerMessage和连接筛选目标
    target: WebSocketMessageTarget--发送范围
    message: ServerMessage--待序列化协议事件
    impl Event for WebSocketMessageSend
```

# types

types 放除事件和错误外的其余类型。

## 类型

crate公开：
```text
BackendStateReportCache：后端状态报告缓存，crate公开Resource--阻止未变化快照在事件驱动Runtime中形成自唤醒循环
    state: Option<BackendStateDto>--上次同步的完整状态
    recipients: Vec<u64>--上次报告时匹配backend_state目标的连接ID
    last_error: Option<String>--最近一次转换错误，用于相同错误只记录一次
    impl Resource for BackendStateReportCache

PendingMclCommandResponses：MCL命令响应等待表，crate公开Resource--保存已发送MCL命令的一次性响应接收器
    commands: Vec<PendingMclCommandResponse>--等待中的MCL命令响应
    impl Resource for PendingMclCommandResponses

PendingMclCommandResponse：单条MCL命令响应，crate公开结构体
    id: String--请求ID
    connection_id: WebSocketConnectionId--来源连接
    response: Mutex<mpsc::Receiver<Result<serde_json::Value, String>>>--命令响应接收器
```

空板块：
    某个板块在本 crate 中没有内容时，对应的源文件保持为空并仍然保留模块声明
    空文件不是遗漏，而是"该板块在此 crate 中确实没有定义"的表示
    保留文件与模块声明是为了让六个板块的位置固定，新增内容时不必再改结构

# error

```text
DtoPlugin 不定义错误类型；入站转换失败和出站发送失败只记录warn日志。
```

## 逻辑

```text
入站：
    WebSocketMessageReceived
        -> dto_route_system
        -> handle_inbound_message
        -> ClientMessage { type, id, message }
        -> message DTO::into_domain
        -> 直接发送领域事件

MCL响应：
    MclPlugin完成命令
        -> dto_route_system
        -> handle_pending_mcl_responses
        -> MclCommandResult发回原连接

出站：
    外部可见领域事件
        -> collect_external_events_system
        -> handle_collect_external_events构造ServerMessage
        -> 包装WebSocketMessageSend
    Workspace、Agent和Memory当前状态
        -> report_backend_state构造BackendStateDto并与缓存比较
        -> 内容或接收连接集合未变化时结束
        -> StateSync发送给backend_state目标
    LogPlugin::TracingStream
        -> forward_logs异步订阅
        -> LogRecordDto转换
        -> 按logs目标发送WebSocketMessageSend
    其他Plugin也可直接发送WebSocketMessageSend
        -> dto_route_system序列化ServerMessage
        -> WebSocketConnections按target筛选
        -> 构造WebSocketMessageSender并发送Text
```

## 边界

```text
DtoPlugin负责：WebSocket帧、JSON信封、DTO转换调用、入站领域事件发送、外部可见领域事件投影、完整状态快照、普通出站消息的发送终端构造、结构化日志转发、ServerMessage序列化和连接筛选。
DtoPlugin不负责：manager路由、Agent Entity创建、Memory、Tool、Inference和资源权限。
Protocol负责：XxxDto及其IntoDomain/FromDomain转换，包括使用World进行只读身份投影。
LogPlugin负责：生成TracingStream，不感知DTO、WebSocket或连接目标。
WorkspacePlugin负责：消费Workspace领域命令、解析Workspace和Agent逻辑身份。
AgentPlugin负责：处理已经携带Entity的AgentMessage。
```

中止网络关联使用内部唯一 operation_id，并核对 agent Entity、resource_id、workspace、完整 VM instance 与 expected_turn_id；只回复原 Client 实例与连接，断连清除关联但不撤回已受理中止。Working 期间拒绝 INJECT ? 中递归包含 user 的消息参数；工具可见性、sandbox 等非用户数组不被此守卫误拦截。
