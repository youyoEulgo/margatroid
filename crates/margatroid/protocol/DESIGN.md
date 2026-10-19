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
    行为：反序列化JSON，校验信封、字段形状及大小限制。

encode_server_message(message: &ServerMessage) -> Result<String, ProtocolError>
    行为：将纯网络消息序列化为JSON。
```

# types pub

## 类型

公开：
```text
ClientMessage：入站消息，公开枚举--JSON信封为type、id、message
ServerMessage：出站消息，公开枚举--承载回执、状态、历史、日志和流分片
AddressDto：公开字符串值--外部实体地址或文件资源定位字符串，具体含义由字段决定
```

```text
WorkspaceReferenceDto
    id: AddressDto
    name: String
    project_root: String
WorkspaceDefinitionDto
    id/name/project_root
    agents: Vec<WorkspaceAgentDefinitionDto>
WorkspaceAgentDefinitionDto
    id/name/driver/home_root
    不包含manager、数据库路径或运行状态
WorkspaceInfoDto
    id/name/project_root
    agents: Vec<AddressDto>
    member_failures: Vec<{agent,driver,error}>，空省略
BackendStateDto
    workspace_definitions: Vec<WorkspaceDefinitionDto>--启动时加载的内存配置
    workspaces: Vec<WorkspaceInfoDto>，真实运行实例
    agents: Vec<AgentStateDto>
    histories: Vec<AgentHistoryDto>
    drivers: Vec<AddressDto>，缺字段默认[]，空可省略
AgentStateDto
    workspace/agent/agent_instance
    status: creating|idle|working|stopping
    turn: Option<String>
    sandbox: Option<AddressDto>
    queued_commands/pending_command
    context_window_tokens: Option<u64>
    error: Option<ProtocolErrorDto>
ProjectDirectoryPageDto
    path: 规范绝对目录
    parent: Option<规范绝对目录>
    directories: Vec<{name,path}>
    next_cursor: Option<String>
```

```text
AgentMessageDto
    System {content:String}
    Inject {messages:Vec<AgentMessageDto>}
    User {content:String,images:Vec<UserImageDto>}
    Assistant {reasoning:Option<String>,content:Option<String>,
               replay:Option<Value>,tool_calls:Vec<ToolCallDto>,
               usage:Option<TokenUsageDto>,stop_reason:Option<StopReasonDto>}
    ToolCall {resource_id:Option<AddressDto>,tool_call_id,tool_name,
              content:String,images:Vec<UserImageDto>,failed:bool}
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

# error

## 类型

公开：
```text
ProtocolError：网络格式错误，公开类型--JSON、字段、长度或消息形状不合法
ProtocolErrorDto：对外错误，公开结构体
    kind: String
    message: String--最多512个UTF-8字节
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
