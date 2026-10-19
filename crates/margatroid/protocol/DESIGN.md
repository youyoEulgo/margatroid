# Protocol

## 文档状态与职责

当前 catalog、注册启动、成员增删、工作区创建/改名/删除、成员改名、Driver 发现及后端目录浏览均已实现并完成恢复验证。验证仅覆盖已执行的 scoped、真实存储集成和隔离数据浏览器场景，不构成所有故障或环境的普遍保证。

Protocol 只定义 JSON 传输值、纯转换及只读受检身份解析；不扫描磁盘、不执行 SQLite、不管理 VM、Sender、取消令牌或活动。共享领域值/事件来自 margatroid_types；DTO 入口负责连接上下文与事件分发，Workspace/loader/存储能力负责实际工作。

# lib

## 转换与寻址

```text
FromDomain / IntoDto
    消费提供的纯数据和必要的只读World上下文，返回DTO或ProtocolError
IntoDomain / FromDto
    校验传输形状并构造共享领域输入；不得隐藏IO或执行副作用
resolve_workspace(world, WorkspaceReferenceDto)
    核对规范ID/name/project_root、唯一Workspace与完整Entity
resolve_agent(world, workspace, ResourceIdDto)
    要求显式Agent ID；核对属于当前Workspace及其完整成员Entity
    不接受null，不使用manager或首成员回退，不接受宿主Entity index
```

Agent/Workspace名称仍参与公开ResourceID，重命名需由领域执行已规定的持久身份迁移；Protocol不另造稳定ID，不盲目替换历史字符串。Driver规范前缀为driver，不引入agent-driver别名。

# system

无System。客户端先注册，后发业务请求；Protocol不安装扫描/迁移/轮询系统。源身份绑定、网络pending、原始请求ID和内部operation ID分离归DtoPlugin。

# handler

## 纯输入校验

信封为 `{type,id,message}`。id非空且不超过512 UTF-8字节；MCL command非空且最多65536字节，arguments各项为数组。操作name/new_name使用同一安全逻辑名称规则；项目根存在性、规范目录和revision事实由受检存储服务在实际动作处再验证。

变更/注册启动不接受额外definition、YAML、history_path/memory_path。字段统一snake_case；`expected_revision`是明确的源版本。create只有name/project_root，无客户端revision，后端按目标不存在创建。无效请求要有回执或协议关闭，不能静默丢弃。

AgentMessageDto递归转换限制深度32、节点4096、非图像序列化大小1MiB；图片另用共享8/5MiB/20MiB预算校验。总WebSocket/DTO入口为32MiB，不能把原消息值1MiB预算当整张图片请求的静默过滤器。允许某消息形状不等于授予伪造工具完成权限。

# events

## 请求与回执目标

```text
connection.register {client_type,name?}
    -> connection.registered {id,client} / connection.register_failed {id,error}
workspace.catalog.query / workspace.catalog.refresh {}
    -> workspace.catalog {id,entries}
workspace.start {workspace,expected_revision}
    -> StartRegisteredWorkspace
    -> workspace.started {id,workspace} / workspace.start_failed {id,error}
workspace.stop {workspace:WorkspaceReferenceDto}
    -> workspace.stopped {id,workspace} / workspace.stop_failed {id,error}
workspace.member.add {workspace,expected_revision,agent:{name,driver,home_root?}}
workspace.member.remove {workspace,expected_revision,agent:ResourceIdDto}
workspace.create {name,project_root}
workspace.rename {workspace,expected_revision,new_name}
workspace.delete {workspace,expected_revision}
workspace.member.rename {workspace,expected_revision,agent:ResourceIdDto,new_name}
    -> 共享对应生命周期事件
    -> workspace.member.result {id,entry,error?}
workspace.paths.query {path?,cursor?}
    -> QueryProjectDirectories / ProjectDirectoryPage
    -> workspace.paths {id,path,parent,directories:[{name,path}],next_cursor?}
driver.catalog.query / driver.catalog.refresh {}
    -> loader清单共享查询/刷新事件
    -> driver.catalog {id,drivers,errors:[{path,message}]}
state.query {}
    -> state.sync {state}
history.query {workspace,agent:ResourceIdDto}
    -> history.result {id,history}
mcl.command {workspace,agent:ResourceIdDto,command,arguments}
    -> mcl.command_result {id,source,target,result,history_error}
agent.turn.abort {workspace,agent:ResourceIdDto,turn_id}
    -> agent.turn.abort_result {id,workspace,agent,turn_id,result}
```

workspace路径/Driver查询已实现。目录浏览path缺省daemon HOME；绝对目录规范化、可读性、排序、有界每页200及cursor版本绑定由注入的ProjectDirectoryBrowser能力执行，不在Protocol I/O，不读取文件内容或创建目录。Driver目录来自有效已安装包，包含未挂载包，query读快照、refresh异步复验；真正加载时再验。缺包、权限、失效cursor和扫描失败均返回关联诊断。

统一变更回执：保存/改名/成员增删成功entry为最新目录项且无error；失败entry=null且有error；**工作区删除成功entry=null且无error**。不因目录无被删项合成失败，不承诺独立workspace.deleted消息或暴露WorkspaceDeletion内部路径。持久成功与运行成员启动失败分别表达。

所有关联回执只发起连接；主动catalog/状态更新依配置投递。内部operation_id区别于网络id，避免跨客户端同id串回执；断连不等同撤回已提交操作。

# types

## 目录、实例与选择

```text
WorkspaceReferenceDto
    id: ResourceIdDto
    name: String
    project_root: String
WorkspaceDefinitionDto
    id/name/project_root
    agents: Vec<WorkspaceAgentDefinitionDto>
WorkspaceAgentDefinitionDto
    id/name/driver/home_root
    不包含manager、数据库路径或运行状态
WorkspaceCatalogEntryDto
    id/name/path/revision
    definition: Option<WorkspaceDefinitionDto>
    error: Option<String>
    lifecycle: stopped|starting|running|stopping|failed
WorkspaceInfoDto
    id/name/project_root
    agents: Vec<ResourceIdDto>
    member_failures: Vec<{agent,driver,error}>，空省略
BackendStateDto
    catalog: Vec<WorkspaceCatalogEntryDto>，旧值缺字段默认[]
    workspaces: Vec<WorkspaceInfoDto>，真实运行实例
    agents: Vec<AgentStateDto>
    histories: Vec<AgentHistoryDto>
    drivers: Vec<ResourceIdDto>，缺字段默认[]，空可省略
AgentStateDto
    workspace/agent/agent_instance
    status: creating|idle|working|stopping
    turn: Option<String>
    sandbox: Option<ResourceIdDto>
    queued_commands/pending_command
    context_window_tokens: Option<u64>
    error: Option<ProtocolErrorDto>
ProjectDirectoryPage（共享纯值，定义于 discovery 模块）
    path: 规范绝对目录
    parent: Option<规范绝对目录>
    directories: Vec<{name,path}>
    next_cursor: Option<String>
```

目录定义有效性与runtime生命周期独立，坏定义或已消失home不能隐去live实例。空成员工作区合法、可启动、可投影；无manager预检。灰/绿/红是UI派生显示，不作为新的Agent生命周期；运行错误保留error/member_failures，不能假造Idle或failed Agent枚举。

`state.sync.drivers`只输出规范Driver地址，不输出源码、依赖、模型设置或凭据。未能扫描最新安装包时错误与最后确认快照并存，不将已挂载列表当有效安装目录。

## 消息、历史与结果

```text
AgentMessageDto
    System {content:String}
    Inject {messages:Vec<AgentMessageDto>}
    User {content:String,images:Vec<UserImage>}
    Assistant {reasoning:Option<String>,content:Option<String>,
               replay:Option<Value>,tool_calls:Vec<ToolCall>,
               usage:Option<TokenUsage>,stop_reason:Option<StopReason>}
    ToolCall {resource_id:Option<ResourceIdDto>,tool_call_id,tool_name,
              content:String,images:Vec<UserImage>,failed:bool}
    Error {message:String}
MclCommandDto
    workspace/agent/command/arguments；id只在外层信封
MclCommandResultDto
    id/source/target/result/history_error
HistoryMessageDto
    key/sequence/turn/kind/content/payload/created_at_ms
StreamDelta（wire契约，无需虚构同名Rust结构体）
    type为agent.message.delta或agent.message.reasoning_delta
    id/agent/agent_instance/sequence/content，无kind字段
```

UserImage为media_type及原始标准base64 data，不是data URL；PNG/JPEG/GIF/WebP，单条User或ToolCall最多8张、单张5MiB解码、合计20MiB。validate_user_images和deserialize_user_images执行预算/base64/魔数检查，错误不含载荷；不执行完整图像解码。空images省略，旧缺字段为[]。字段在消息、Lua、持久Block/历史中保留，不混入content。

replay缺字段None，Protocol只透传含api_format/items的不透明JSON；Messages签名/遮蔽thinking和Responses reasoning/加密元数据不由Protocol伪造。跨格式兼容和API图片形状由InferencePlugin判断。

TOOL出站只含resource_id/alias/description/input_schema，SANDBOX只含resource_id，不能把安全投影当可信入站执行快照。工具真实执行结果由对应领域oneshot提供。推理/工具只返回结果，显式history_append决定消息历史；MCL指令审计自动追加但不自动形成message历史。

历史sequence按list顺序1-based，created_at_ms保留真实记录时间；旧0表示未知。正文从content、类型与reasoning/images/usage/stop_reason/replay等从payload还原；Inject保持完整嵌套。history.query当前依赖live Agent句柄，不隐式承诺离线历史读取。历史事实不因重命名全局字符串替换；新身份查询关联来自迁移后正确绑定的元数据/句柄。

## 中止与流终态

独立abort验证当前完整实例/预期turn，只取消匹配turn_cancel；受理清turn但保持Working。正常Driver finish_turn核对真实清理再Idle。daemon将PRE_UPDATE中止排在UPDATE域admission前；插件默认计划不构成普遍排序保证。

用户取消推理先abort+join本地HTTP/SSE worker，再返回累计reasoning/content，inner/outer stop_reason一致Cancelled。无输出为空，无tool_calls/replay/虚构usage；根关闭/owner abort/timeout/receiver关闭/cleanup失败保持各自原因。工具取消保留原ID/顺序/资源、failed及取消说明，已完成项不改写。

cancelled Assistant只允许显式历史，不得进入infer上下文（包括Inject）；工具结果仍正常保留。前端取消普通渲染、空值不补文案，点击显示屏障不伪造清理完成。真实finished只提供渲染终态，oneshot/MCL结果为完成权威；本地断连不保证远端停止计费。

# error

ProtocolErrorDto为kind/message，message最多512 UTF-8字节，无凭据或图片正文。分别报告未注册、非法参数、目标不存在、重复在途、修订冲突、持久失败、清理失败及提交未知；所有可关联错误保留原网络id。目录单项错误保留条目，不让一个坏文件阻断其他条目。DTO入口超32MiB关闭1009，无法关联的非法JSON关闭1007；有合法id结构错误返回request.failed。

## 验证与范围

当前 DTO/Protocol scoped 验证合计 34 passed，覆盖恢复的 catalog、registered-start、成员与 CRUD/discovery 路由、精确网络 ID、generation 和错误关联。领域存储迁移与真实浏览器交互另有独立验证，不能由 DTO 测试替代。
