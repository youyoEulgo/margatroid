# 伪代码格式

```text
板块使用一级标题；类型、函数使用二级标题。
列出公开字段、签名与直接行为，不展开领域执行流程。
不写Rust Attribute；持有关系使用树形图。
```

# 板块格式

```text
# lib       src/lib.rs     协议公开入口
# types pub src/types.rs   纯网络信封与DTO
# error     src/error.rs   网络错误类型
```

# lib

## 函数

公开：
```text
parse_client_message(text: &str) -> Result<ClientMessage, ProtocolError>
    行为：反序列化JSON，校验type、请求id、message对象、字段形状和字节预算；拒绝未知type及已删除旧字段，不负责领域受理。

encode_server_message(message: &ServerMessage) -> Result<String, ProtocolError>
    行为：按具体variant序列化顶层字段；校验成功/失败互斥、必需结果及消息预算；sync不生成请求id。

valid_request_id(id: &str) -> bool
    行为：检查非空白、无控制字符及512字节上限。

validate_server_value(value: &serde_json::Value) -> Result<(), ProtocolError>
    行为：检查严格wire type、回执成功/失败互斥、查询载荷、分页游标presence、nullable字段、记录顺序及完整消息序号关联。

validate_value / validate_internal_message_fields / require_fields
    行为：检查递归预算、媒体、已删除旧消息字段、真实正sequence及必有字段；不推断领域权限或实例受理。

logical_name / validate_path / validate_target / validate_address / validate_image
    行为：检查纯网络值形状、目标kind、路径及标准媒体编码，不依赖领域core。
```

# types pub

## 类型

公开：
```text
AddressDto = String

ClientMessage：入站枚举，每个variant均为type、id和message信封
    ConnectionRegister { id: String, message: RegisterConnectionDto }
    WorkspaceReq { id: String, message: EmptyDto }
    WorkspaceStart { id: String, message: WorkspacePathDto }
    WorkspaceStop { id: String, message: WorkspacePathDto }
    WorkspaceRestart { id: String, message: WorkspacePathDto }
    WorkspaceReload { id: String, message: EmptyDto }
    AgentReq { id: String, message: EmptyDto }
    AgentStart { id: String, message: AgentStartDto }
    AgentStop { id: String, message: AgentTargetDto }
    AgentRestart { id: String, message: AgentTargetDto }
    AgentTurnReq { id: String, message: AgentTargetDto }
    AgentTurnAbort { id: String, message: AgentTurnAbortDto }
    AgentTaskReq { id: String, message: AgentTargetDto }
    AgentHistoryReq { id: String, message: AgentRecordsQueryDto }
    AgentLogReq { id: String, message: AgentRecordsQueryDto }
    AgentMclReq { id: String, message: AgentRecordsQueryDto }
    MclExecute { id: String, message: MclExecuteDto }

ServerMessage：出站枚举，type之外的字段位于顶层，不包message
    ConnectionRegisterResult { id: String, success: bool, client: Option<ClientInfoDto>, error: Option<ProtocolErrorDto> }
    WorkspaceRes { id: String, success: bool, workspaces: Option<Vec<WorkspaceInfoDto>>, error: Option<ProtocolErrorDto> }
    WorkspaceSync { workspaces: Vec<WorkspaceInfoDto> }
    WorkspaceStartResult { id: String, success: bool, error: Option<ProtocolErrorDto> }
    WorkspaceStopResult { id: String, success: bool, error: Option<ProtocolErrorDto> }
    WorkspaceRestartResult { id: String, success: bool, error: Option<ProtocolErrorDto> }
    WorkspaceReloadResult { id: String, success: bool, error: Option<ProtocolErrorDto> }
    AgentRes { id: String, success: bool, agents: Option<Vec<AgentStateDto>>, error: Option<ProtocolErrorDto> }
    AgentSync { agents: Vec<AgentStateDto> }
    AgentStartResult { id: String, success: bool, error: Option<ProtocolErrorDto> }
    AgentStopResult { id: String, success: bool, error: Option<ProtocolErrorDto> }
    AgentRestartResult { id: String, success: bool, error: Option<ProtocolErrorDto> }
    AgentTurnRes { id: String, success: bool, state: Option<AgentTurnStateDto>, error: Option<ProtocolErrorDto> }
    AgentTurnSync { address: AddressDto, turn: Option<String> }
    AgentTurnAbortResult { id: String, success: bool, error: Option<ProtocolErrorDto> }
    AgentTaskRes { id: String, success: bool, address: AddressDto, tasks: Option<Vec<AgentTaskDto>>, error: Option<ProtocolErrorDto> }
    AgentTaskSync { address: AddressDto, tasks: Vec<AgentTaskDto> }
    AgentHistoryRes { id: String, success: bool, address: AddressDto, records: Option<Vec<HistoryMessageDto>>, next_before_sequence: Option<u64>, next_after_sequence: Option<u64>, error: Option<ProtocolErrorDto> }
    AgentLogRes { id: String, success: bool, address: AddressDto, records: Option<Vec<AgentLogRecordDto>>, next_before_sequence: Option<u64>, next_after_sequence: Option<u64>, error: Option<ProtocolErrorDto> }
    AgentLogSync { address: AddressDto, records: Vec<AgentLogRecordDto> }
    AgentMclRes { id: String, success: bool, address: AddressDto, records: Option<Vec<AgentMclRecordDto>>, next_before_sequence: Option<u64>, next_after_sequence: Option<u64>, error: Option<ProtocolErrorDto> }
    AgentMclSync { address: AddressDto, records: Vec<AgentMclRecordDto> }
    AgentMessageChunkSync { address: AddressDto, turn: String, generation: String, sequence: u64, delta: MessageDeltaDto }
    AgentMessageFullSync { address: AddressDto, turn: Option<String>, generation: Option<String>, history_sequence: Option<u64>, message: AgentMessageDto, error: Option<ProtocolErrorDto> }
    MclExecuteResult { id: String, success: bool, data: Option<MclExecuteDataDto>, error: Option<ProtocolErrorDto> }

    id(&self) -> Option<&str>
        行为：result/res返回原请求id，全部sync返回None。
    impl Serialize
        行为：转换私有ServerWire为JSON，成功分页补齐必有nullable游标，然后validate_server_value再序列化；失败分支省略全部成功字段。
    impl Deserialize
        行为：先读取JSON并validate_server_value，再反序列化严格ServerWire转换为公开variant，不接受旧type或额外字段。
    行为：
        command result始终回复原id；状态类成功仅success=true，失败success=false及error，不重复状态或目标
        连接注册成功必须提供client；MCL成功的非Unit值提供data，Unit省略data；失败省略成功载荷
        res成功提供对应集合或state，合法空集合仍返回；失败不提供集合/state/分页游标，不能用空数组冒充失败
        query成功的分页游标必须出现，无下一页时为null；失败时字段省略，Option字段序列化受success分支约束
        sync不带请求id；完整状态与新增记录使用不同variant，不保留BackendStateDto、state.*、log.sync或旧别名
```

```text
ServerWire：私有serde中间枚举，与ServerMessage同一variant及字段，一对一From转换；只用于严格信封序列化，不是另一份业务状态
ClientMessage.id(&self) -> &str
    行为：返回该请求的id，不生成或改写。
EmptyDto：空结构体
RegisterConnectionDto
    client_type: String
    name: Option<String>
ClientInfoDto
    address: AddressDto
    client_type: String
WorkspacePathDto
    path: String
AgentStartDto
    workspace: AddressDto
    name: String
AgentTargetDto
    address: AddressDto
AgentTurnAbortDto
    address: AddressDto
    turn: String
AgentRecordsQueryDto
    address: AddressDto
    before_sequence: Option<u64>
    after_sequence: Option<u64>
    limit: Option<usize>
    行为：
        双边界要求after_sequence < before_sequence，sequence不解释为offset或条数
        无下界时读取上界前最近limit条；有下界时读取区间内最早limit条；响应始终sequence升序
        三种记录共享查询形状但各自访问所属记录域，日志序号不能用作消息或MCL游标
MclExecuteDto
    address: AddressDto
    command: String
    arguments: Vec<serde_json::Value>
```

```text
WorkspaceInfoDto
    path: String
    definition: WorkspaceDefinitionDto
    status: String
    address: Option<AddressDto>
    error: Option<ProtocolErrorDto>
WorkspaceDefinitionDto
    name: String
    project_root: String
    agents: Vec<WorkspaceAgentDefinitionDto>
WorkspaceAgentDefinitionDto
    name: String
    driver: String
    home_root: String
    行为：仅配置成员定义，不附带成员active、运行地址、status或member_failures
AgentStateDto
    address: AddressDto
    workspace: AddressDto
    status: String
    tools: Vec<AgentToolDto>
    error: Option<ProtocolErrorDto>
    行为：只投影实际已激活实例；停止完成后从集合移除，不为未激活定义成员生成占位项
AgentToolDto
    alias: String
    path: String
    description: String
    input_schema: serde_json::Value
    owner_task_id: Option<String>
    binding: Option<serde_json::Value>
    行为：完整保留Tools中全部工具记录及属性，Tools为空返回空数组，不混入运行任务
AgentTurnStateDto
    address: AddressDto
    turn: Option<String>
AgentTaskStateDto
    address: AddressDto
    tasks: Vec<AgentTaskDto>
AgentTaskDto
    task_id: String
    call_id: String
    alias: String
    returned: bool
    output: Vec<serde_json::Value>
    error: Option<ProtocolErrorDto>
    行为：完整活跃集合包含未return及已return未finish任务；finish真实移除，不保留完成缓存
```

```text
AgentMessageDto：厂商无关消息枚举，各variant均有sequence:Option<u64>
    System { sequence: Option<u64>, content: String }
    User { sequence: Option<u64>, content: String, data: Vec<UserImageDto> }
    Assistant { sequence: Option<u64>, request_id: Option<String>, reasoning: Option<String>, content: Option<String>, tool_calls: Vec<ToolCallDto>, usage: Option<TokenUsageDto>, stop_reason: Option<String> }
    ToolCall { sequence: Option<u64>, tool_call_id: String, tool_name: String, content: String, data: Vec<UserImageDto>, failed: bool }
    Inject { sequence: Option<u64>, messages: Vec<AgentMessageDto> }
    Error { sequence: Option<u64>, message: String, request_id: Option<String> }
    行为：
        JSON type分别为system/user/assistant/tool_call/inject/error；缺省sequence不输出，存在时必须为真实持久历史正序号
        data数组可省略为空；reasoning和content各保存一份，禁止replay、api_format和images兼容字段
        数据库sequence/role/content/data/tool_call/payload由存储codec可逆重建本枚举；SQL payload只含剩余字段，不是wire完整消息
        inject是显式消息包装，其messages保持结构化数组；角色转换及解包不属于协议解析器
UserImageDto
    media_type: String
    data: String
ToolCallDto
    id: String
    tool_name: String
    arguments: String
TokenUsageDto
    input_tokens: u64
    output_tokens: u64
    cache_hit_tokens: Option<u64>
HistoryMessageDto
    sequence: u64
    turn: Option<String>
    kind: String
    content: String
    payload: AgentMessageDto
    created_at_ms: u64
AgentLogRecordDto
    sequence: u64
    created_at_ms: u64
    level: String
    source: String
    stage: String
    message: String
    details_json: Option<serde_json::Value>
    context_json: Option<serde_json::Value>
    行为：投影turn、task、call及alias等业务关联，过滤driver_instance等内部VM标识。
    origin: Option<String>
    raw_error: Option<String>
    traceback: Option<String>
AgentMclRecordDto
    sequence: u64
    created_at_ms: u64
    source: AddressDto
    turn: Option<String>
    content: String
    payload: serde_json::Value
    行为：投影MCL审计业务字段，移除source_instance及其他内部VM标识。
MessageDeltaDto
    content: Option<String>
    reasoning: Option<String>
    行为：只表示新增片段，不能替代完整AgentMessage，不使用分片序号冒充历史序号
MclExecuteDataDto：按kind标记的公开枚举，各variant用value承载业务值
    Messages { value: Vec<AgentMessageDto> }
    Config { value: serde_json::Value }
    Import { value: MclImportResultDto }
    Inference { value: MclInferenceResultDto }
    BlockSnapshots { value: serde_json::Value }
MclImportResultDto
    kind: String
    alias: String
MclInferenceResultDto
    message: AgentMessageDto
    stop_reason: String
```

# error

## 类型

公开：
```text
ProtocolError：网络格式错误，公开类型
ProtocolErrorDto：对外错误，公开结构体
    code: String
    message: String
    details: Option<serde_json::Value>
```

# 持有关系

```text
ClientMessage / ServerMessage
└── 协议DTO
    ├── 字符串、数字、数组和纯值
    └── 协议自身的嵌套DTO

protocol
└── 标准库与serde等通用库
```

# 中止与任务查询契约

```text
agent.turn.abort使用address、turn精确选择原目标，agent.turn.abort.result只回复原id及success/error，不重复回合状态。
中止请求由DTO进入既有控制路径，先清turn，再取消当前推理、强制return并终止未return工具；已return未finish任务保留。
中止回执不等同于整个Agent停止或工具全部清理完成；agent.turn.sync交付实际回合状态，不以sync完成命令等待。
agent.task.req/res/sync使用address；res和sync均提供全部活跃任务和完整输出窗口，不分块、不带removed补丁。
任务查询和同步不调用脚本task_read、不推进其共享游标、不清除replaced标记、不触发finish。
finish真实清理后移除任务，集合不提供completed=true条目或完成缓存；后续read/send/finish返回TaskMissing。
agent.message.chunk.sync与agent.message.full.sync是生成内容通知；full携带完整消息和可选持久序号，不恢复独立finished消息。
full.history_sequence非null时必须等于message.sequence；null不伪造序号，生成失败或落库失败通过full.error表达。
```
