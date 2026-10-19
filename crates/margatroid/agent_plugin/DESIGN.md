# 伪代码格式

```text
板块：一级标题，一行注明core与plugin文件，不创建空模块。
类型、函数：二级标题，区分公开、crate公开与私有。
类型列出字段；函数列出签名及直接行为。
System写明处理事件、产出事件，只收集事件并路由至Handler。
属性不写Rust Attribute；持有关系使用树形图。
```

# 板块格式

```text
# lib        agent/core/src/lib.rs；agent/plugin/src/lib.rs  Agent自身扩展；Plugin安装
# components agent/core/src/components.rs                    Agent组件
# events     agent/core/src/events.rs                        启停、重启及异常事件
# system     agent/plugin/src/system.rs                      事件收集与路由
# handler    agent/plugin/src/handler.rs                     同步受理、异步流程与结果收口
# types      agent/plugin/src/types.rs                       流程参数与闭包输出
# types pub  agent/core/src/types.rs                         AgentState
# error      agent/core/src/error.rs                         AgentError
```

# lib

## 类型

公开，定义于agent_core：
```text
WorldAgentExt：Agent访问扩展，公开trait
    agent_state(&self, agent: Entity) -> Result<&AgentState, AgentError>
        行为：读取Agent状态，目标不存在返回错误。
    agent_start_turn(&mut self, agent: Entity) -> Result<String, AgentError>
        行为：仅Idle允许；同步生成turn并转Working。
    agent_finish_turn(&mut self, agent: Entity, turn: &str) -> Result<(), AgentError>
        行为：核对当前回合关联；Working转Idle并清空turn；拒绝旧回合完成。
    agent_abort_turn(&mut self, agent: Entity, turn: &str) -> Result<(), AgentError>
        行为：仅Working且turn匹配时同步清空turn，保持Working；否则返回错误。
    impl WorldAgentExt for World
        行为：只访问Agent自身组件，不操作其他领域。
```

公开，定义于agent_plugin：
```text
AgentPlugin：生命周期插件，公开类型
    schedule: String--启停System所属计划
    impl Default for AgentPlugin
        行为：使用RuntimePlugin::UPDATE。
    impl Plugin for AgentPlugin
        build(self, app: &mut App)
            行为：
                检查运行设施和计划，拒绝重复安装。
                在同一schedule依次add_system：stop_agent_system、restart_agent_system、start_agent_system。
                安装异常System；各启停System收集自己的闭包结果。
                使用mecs按挂载顺序同步执行的机制，不增加生命周期排序器或统一请求枚举。
```

# components

## 类型

公开：
```text
Agent：Agent领域组件，公开结构体
    name: String
    workspace: Entity
    state: AgentState
    turn: Option<String>
    impl Component for Agent

```

# events

## 类型

公开：
```text
StartAgent：启动请求，公开事件
    id: String
    workspace: Entity
    name: String
    impl Event for StartAgent

StopAgent：关闭请求，公开事件
    id: String
    agent: Entity
    impl Event for StopAgent

RestartAgent：重启请求，公开事件
    id: String
    agent: Entity
    impl Event for RestartAgent

StartAgentResult / StopAgentResult / RestartAgentResult：操作结果，公开事件
    id: String
    result: Result<Entity, AgentError>
    impl Event

AgentException：异常输入，公开事件
    id: String
    agent: Entity
    error: AgentError
    impl Event for AgentException
```

# system

## 函数

crate公开：
```text
stop_agent_system(world: &mut World)
    处理事件：StopAgent、Result<StopOutput, StopError>
    产出事件：关闭异步闭包、StopAgentResult
    行为：收集请求与闭包结果，路由到对应Handler。

restart_agent_system(world: &mut World)
    处理事件：RestartAgent、Result<RestartOutput, RestartError>
    产出事件：重启异步闭包、RestartAgentResult
    行为：收集请求与闭包结果，路由到对应Handler。

start_agent_system(world: &mut World)
    处理事件：StartAgent、Result<StartOutput, StartError>
    产出事件：启动异步闭包、StartAgentResult
    行为：收集请求与闭包结果，路由到对应Handler。

agent_exception_system(world: &mut World)
    处理事件：AgentException及领域异常结果
    产出事件：异常处理结果
    行为：收集事件，路由至异常Handler。
```

# handler

## 函数

crate公开：
```text
handle_start_agent(world: &mut World, request: StartAgent)
    行为：校验Workspace和成员创建请求；同步创建Agent Entity并挂基础组件，置Starting，再提交start_flow；拒绝时发送启动错误。

handle_stop_agent(world: &mut World, request: StopAgent)
    行为：按白名单同步转Stopping；关闭新活动受理，再提交stop_flow；拒绝时发送关闭错误。

handle_restart_agent(world: &mut World, request: RestartAgent)
    行为：按白名单同步转Restarting；关闭新活动受理，再提交restart_flow；拒绝时发送重启错误。

handle_start_result(world: &mut World, result: Result<StartOutput, StartError>)
handle_stop_result(world: &mut World, result: Result<StopOutput, StopError>)
handle_restart_result(world: &mut World, result: Result<RestartOutput, RestartError>)
    行为：
        核对Entity及对应生命周期状态；忽略失效结果。
        启动或重启成功：转Idle，发送对应结果。
        关闭成功：销毁Entity，发送关闭结果。
        失败：有完整清理证明则销毁Entity，否则保留宿主及当前生命周期状态；发送对应错误。

```

私有：
```text
start_flow(context: FlowContext) -> Future<Output = Result<StartOutput, StartError>>
    行为：依次发送数据库准备、运行组件初始化、Driver启动请求并等待各自oneshot；成功返回；失败清理已建立资源并返回清理事实。

stop_flow(context: FlowContext) -> Future<Output = Result<StopOutput, StopError>>
    行为：请求Driver停止和执行取消；等待全部活动及MCL收口；返回清理结果，不销毁协调Entity。

restart_flow(context: FlowContext) -> Future<Output = Result<StartOutput, StartError>>
    行为：await stop_flow，成功后await start_flow；全程Restarting，不发公开StopAgent或StartAgent事件。
```

# types

## 类型

私有：
```text
FlowContext：闭包上下文
    id: String--原请求ID
    agent: Entity
    events: RuntimeEventSender
    parameters: FlowParameters--owned启动材料

StartOutput / StopOutput / RestartOutput：对应闭包的成功载荷
    id: String--原请求ID
    agent: Entity

StartError / StopError / RestartError：对应闭包的失败载荷
    id: String--原请求ID
    agent: Entity
    error: AgentError
    cleanup_completed: bool

```

# types pub

## 类型

公开：
```text
AgentState：生命周期状态，公开枚举
    Starting--仅允许匹配当前启动操作的步骤与结果
    Idle--允许开始回合、StopAgent、RestartAgent
    Working--允许结束或中止当前回合、StopAgent、RestartAgent
    Restarting--仅允许匹配当前操作的步骤与结果
    Stopping--仅允许匹配当前操作的步骤与结果

```

# error

## 类型

公开：
```text
AgentError：生命周期错误，公开类型
    kind: AgentErrorKind
    message: String
    impl Display + std::error::Error

AgentErrorKind：错误类别，公开枚举
    InvalidRequest | AgentMissing | StateRejected
    StartFailed | StopFailed | RestartFailed | CleanupFailed | ReplyClosed
```

# 持有关系

```text
World
└── Agent Entity
    ├── Address（address）
    ├── Agent（agent_core）
    ├── Driver运行组件（Lua领域core）
    ├── ExecutionScope（执行领域core）
    ├── MclQueue（MCL领域core）
    ├── 数据库与历史组件（存储领域core）
    └── 各资源路径集合（所属领域core）

异步闭包
└── FlowContext
    ├── Entity
    ├── owned参数及事件发送能力
    └── 本步骤oneshot接收端
```
