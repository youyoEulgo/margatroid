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

本文协议已迁移到共享AgentMessage、ResourceID及MclInstruction，不保留旧邮箱、默认可见性和Workflow入口。README描述当前wire与构造API。

数据库Block已实现：CREATE BLOCK持久打开、CREATE TEMP BLOCK启动隔离；MclValue新增BlockOpened {block_id, created, temporary}，Lua/网络保持外层BlockOpened标签。轮次结束指令为finish_turn，旧finish已拒绝。DTO在ECS只捕获组件元数据，历史读取在blocking worker，具名共享完成事件唤醒收口；Protocol转换只消费提供的数据，不隐藏SQLite访问。

本轮三类型数组与System消息已实现：MCL仅MESSAGE/TOOL/SANDBOX，显式IMPORT与alias求值；tool_call以单条assistant消息数组为第一参数。网络与Lua资源出站都采用安全投影，TOOL仅resource_id/alias/description/input_schema，SANDBOX仅resource_id，不泄漏脚本路径或policy，也不能将出站对象当作可信入站快照。

实现说明：HistoryMessageDto.sequence按list顺序投影为1-based，created_at_ms保留HistoryRecord创建时间，旧记录的0表示未知。Stream使用完整Entity generation生成agent_instance，并通过agent.message.finished明确终态；终态不是操作完成权威。

显式message历史策略已实现：推理和工具调用只返回结果，所有message历史仅由显式MCL history_append控制。MCL指令审计仍自动记录，审计payload中的消息不作为独立message记录。示例Driver对需要保留的结果另发显式追加指令。

## 用户轮次中止契约（已实现）

独立控制协议已实现：DTO 入口解析当前 Workspace 成员和完整 VM 实例，发送共享 AbortAgentTurn；不恢复旧邮箱、中止查表或全局执行 pending。无 `get_turn`。

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

## 逻辑

```text
协议只定义网络DTO及纯转换，不持有World、Sender、CancellationToken、VM或执行活动。
领域值由共享types提供，Protocol不重复定义ToolCall或消息业务语义。
客户端注册后才接受需要身份的请求，source由真实注册连接绑定而非网络输入。
```

# system

## 逻辑

```text
无System，DTO入口插件负责解析连接消息并发送领域事件。
```

# handler

## 逻辑

```text
纯转换验证字段和序列化形状，不查找Agent、不执行MCL、不写历史。
Workspace/Agent目标解析由受检共享World扩展完成，不能将网络给定Entity index当可信身份。
```

# events

## 逻辑

```text
网络ServerMessage/ClientMessage是传输值，不重复定义领域Event。
连接注册成功回执必须在Client实体可查询后发送，失败回显关联ID与有界错误。
MCL完成只回发起连接，不广播；推理增量向开始时全部webui Sender快照推送。
```

# types

## 类型

公开：
```text
AbortAgentTurnDto：独立回合中止请求，公开结构体
    workspace: WorkspaceReferenceDto
    agent: Option<ResourceIDDto>--None选择manager，不接受宿主Entity
    turn_id: String--必填非空预期轮次
    说明：DTO入口内部解析完整Entity与DriverLuaVmInstance，结果使用agent.turn.abort_result回显id/workspace/agent/turn_id及Result<(), ProtocolErrorDto>。

MclCommandDto：外部单条指令，公开结构体
    id: String
    workspace: WorkspaceReferenceDto
    agent: Option<ResourceIDDto>--None表示Workspace manager
    command: String--原始指令文本
    arguments: Vec<serde_json::Value>--每项为数组，依次对应问号，无问号为空
    说明：转换为MclCommand的command.text及command.arguments，source不可由客户端提供。

MclCommandResultDto：指令结果，公开结构体
    id: String
    source: ResourceIDDto
    target: ResourceIDDto
    result: Result<serde_json::Value, ProtocolErrorDto>--保留MclValue类型与真实结果
    history_error: Option<ProtocolErrorDto>--取消等指令审计附带失败，不代表自动保存结果消息；不改写执行result
    说明：Inference结果保留assistant全部内容及stop_reason，tool_call结果MESSAGE数组保留调用顺序和失败项。
        目标GET结果仅Message/Tool/Sandbox三类数组，TOOL和SANDBOX投影身份与可展示定义，不向客户端泄漏内部script路径、句柄或策略敏感内容
        DTO对资源值的出站投影不等于可作为可信入站快照；入站资源解析仍须通过目标Agent导入绑定验证

AgentMessageDto：消息值投影，公开契约
    System { content: String }--已实现消息类型，保持prompt系统角色；支持消息值不等于开放任意网络入口权限
    Inject { messages: Vec<AgentMessageDto> }
    User { content: String }
    Assistant { reasoning: Option<String>, content: Option<String>, tool_calls: Vec<ToolCall>, usage: Option<TokenUsage>, stop_reason: Option<StopReason> }
    ToolCall { resource_id: Option<ResourceIDDto>, tool_call_id: String, tool_name: String, content: String, failed: bool }
    Error { message: String }
    说明：按共享消息契约递归转换，不存在比AgentMessage更窄的inject内层白名单；限制深度和大小。
        允许消息形状不等于任意入口有权限伪造工具执行完成；工具结果权威来源仍为实际调用oneshot。

ResourceIDDto：资源地址，公开字符串契约
    说明：完整type:scope/name:tag，缺省tag规范化latest，不携带宿主Entity或内部句柄。

WorkspaceReferenceDto：目标工作区，公开结构体
    id: String
    name: String
    project_root: PathBuf

WorkspaceInfoDto：工作区快照，公开结构体
    id: ResourceIDDto
    name: String
    project_root: PathBuf
    manager: ResourceIDDto
    agents: Vec<ResourceIDDto>

AgentStateDto：Agent快照，公开契约
    status: String--creating/idle/working/stopping，与AgentState一致
    turn: Option<String>
    sandbox: Option<ResourceIDDto>
    queued_commands: usize
    pending_command: Option<String>
    context_window_tokens: Option<u64>--与Lua环境相同的Agent启动受理模型窗口快照；没有元数据则省略，不以0冒充
    error: Option<ProtocolErrorDto>--保留的Driver异常
    说明：无默认可见性、Lua程序包装或AgentTokenUsage副本；Token汇总从历史取得。上下文占用取最近assistant的input_tokens加output_tokens，除以上述窗口；cache_hit不重复相加，不把历史累计用量当上下文占用。

HistoryMessageDto：历史投影，公开契约
    key: String--持久记录幂等键
    sequence: i64
    turn: Option<String>
    kind: String
    content: String--message记录的真实正文；error保存错误文本；inject根正文为空
    payload: serde_json::Value--message类型、reasoning、tool_calls、usage、stop_reason等非正文元数据；inject保留完整嵌套消息；MCL保留指令原文及展开参数
    created_at_ms: i64

ClientInfoDto：客户端身份，公开契约
    resource_id: ResourceIDDto
    client_type: String
    name: String

StreamDeltaDto：推理增量，公开契约
    id: String--推理请求ID，不是turn代号
    agent: ResourceIDDto
    agent_instance: String--完整Entity代数生成的不透明实例标识，只用于区分迟到流，不作为入站寻址能力
    sequence: u64
    kind: String--reasoning/content
    content: String
    说明：使用agent.message.reasoning_delta/agent.message.delta类别，流终态明确成功或失败，不作为oneshot完成判据。
```

## 逻辑

```text
客户端命令族：注册、Workspace启动/停止、独立回合中止、MCL命令、状态及历史查询。
移除visibility来源操作、旧turn中止查表及Workflow热插拔旧路由；正常轮次变化走MCL start_turn/finish_turn，agent.turn.abort使用上文已实现的独立共享控制事件。
外部mcl.command一次一条，不允许网络上传Lua源码或取得执行句柄。
状态同步与历史读取不会修改执行状态，不从工具名称恢复全局ResourceMap。
认证和访问权限另属入口控制，source绑定只保证请求来源与连接一致，不冒充完整认证系统。
```

# error

## 类型

公开：
```text
ProtocolErrorDto：网络错误，公开结构体
    kind: String
    message: String--有界、不泄漏凭据
    说明：区分未注册、无目标、非法参数、重复请求及历史记录错误，不丢失关联ID。
```
