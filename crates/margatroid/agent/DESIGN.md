# 伪代码格式

```text
板块：一级标题，一行注明core与plugin文件，不创建空模块。
类型、函数：二级标题，区分公开、crate公开与私有。
类型列出字段；函数列出签名及直接行为。
System分别列出处理事件、接收闭包结果、提交闭包与产出事件，使用Rust类型和函数名；空项使用None。
属性不写Rust Attribute；持有关系使用树形图。
方法使用下列层级；--仅承载简短说明，执行逻辑写入行为。
method(args) -> xxx
    行为：
        xxx
```

# 板块定义

```text
# lib        agent/core/src/lib.rs；agent/plugin/src/lib.rs  公开启动契约与关闭句柄；Plugin安装
# components agent/core/src/components.rs                    Agent组件；其他领域组件只引用其core定义
# events     agent/core/src/events.rs                        StartAgent
# system     agent/plugin/src/system.rs                      同步创建实体、挂载组件和回复
# handler    agent/plugin/src/handler.rs                     关闭句柄的清理行为
# types      agent/plugin/src/types.rs                       关闭操作的内部状态
# types pub  agent/core/src/types.rs                         AgentHandle
# error      agent/core/src/error.rs                         AgentError
```

# lib

## 类型

```text
-- 公开，定义于agent_core
AgentUpdateContext：组件变化和实际来源生命周期探针，公开Resource
    publish: Arc<dyn Fn(&mut World, Entity, AgentUpdateKind) -> Vec<AgentObservationWait> + Send + Sync>
    new(publish: impl Fn(&mut World, Entity, AgentUpdateKind) -> Vec<AgentObservationWait> + Send + Sync + 'static) -> Self
    notify(&self, world: &mut World, owner: Entity, kind: AgentUpdateKind) -> Vec<AgentObservationWait>
        行为：调用装配方注入的回调，core不引用客户端或其他core；普通Changed及Removed不产生异步清理等待。
    impl Resource + Clone

WorldAgentExt：Agent状态和回合的统一更新入口
    agent_ready(&mut self, entity: Entity) -> Result<(), AgentError>
        行为：调用Agent.ready()，实际变化后释放组件借用并调用AgentUpdateContext::notify(Changed)。
    agent_start_turn(&mut self, entity: Entity) -> Result<String, AgentError>
        行为：仅Running受理；已有turn返回原值，否则生成进程内唯一turn并经统一入口提交Changed。
    agent_finish_turn(&mut self, entity: Entity, turn: &str) -> Result<(), AgentError>
        行为：仅Running且当前turn为空或匹配时成功，清空实际turn后通知Changed；不在core等待推理或任务。
    agent_abort_turn(&mut self, entity: Entity, turn: &str) -> Result<(), AgentError>
        行为：仅Running且turn匹配时清空并通知Changed，调用方plugin再取消该回合执行。
    agent_set_turn(&mut self, entity: Entity, turn: Option<String>) -> Result<(), AgentError>
        行为：统一校验并提交turn改变，实际变化后经同一探针发布，不绕过状态与turn约束。
    impl WorldAgentExt for World

-- 公开，定义于agent_plugin
AgentPlugin：Agent启动插件
    impl Plugin for AgentPlugin
        build(self, app: &mut App)
            行为：在RuntimePlugin::UPDATE安装start_agent_system和stop_agent_system。
```

# components

## 类型

```text
-- 公开，定义于agent_core
Agent：Agent自身运行组件
    workspace: Entity--所属Workspace实体
    parent: Entity--实际创建者，Workspace或Agent实体；完整Entity包含generation
    task_id: String--创建者parent的ToolTasks中实际启动任务id，不是模型call_id
    instance: String--本次Agent运行的唯一身份，用于TEMP命名空间及迟到结果校验
    project_root: PathBuf--Driver传入并经校验的完整实际工作目录
    state: AgentState--启动、运行、关闭
    turn: Option<String>--当前回合，仅约束显式携带turn的调用
    ready: tokio::sync::watch::Sender<AgentReadyState>--初始化Pending，关闭时立即置Closed
    wake: Arc<Mutex<AgentWakeSlot>>--wake槽位，独立于Driver Lua VM
    new(workspace: Entity, parent: Entity, task_id: String, project_root: PathBuf) -> Self
        行为：同步创建唯一instance、Starting、turn=None及两个槽位；不读取文件或其他领域组件。
    instance(&self) -> &str
        行为：返回本次运行身份；不能由旧调用方覆盖。
    task_owner(&self) -> (Entity, &str)
        行为：返回parent及task_id，只定位真实创建任务，不从地址字符串猜测任务归属。
    ready(&mut self) -> Result<(), AgentError>
        行为：Starting转为Running并将ready槽位置为Ready；Running重复调用成功；Closing拒绝。公开修改经WorldAgentExt调用，在释放组件借用后通知探针。
    set_state(&mut self, state: AgentState) -> bool
        行为：仅实际变化返回true；Closing关闭新受理；由plugin统一入口通知探针，不持组件借用编码网络消息。
    set_turn(&mut self, turn: Option<String>) -> Result<bool, AgentError>
        行为：校验当前状态，改变turn返回true，相同值返回false；经WorldAgentExt调用并发布实际变化。
    impl Component for Agent
```

# events

## 类型

```text
-- 公开，定义于agent_core
StartAgent<P>：Driver注入方法发出的Agent实体创建事件
    workspace: Entity
    parent: Entity--真实工具发起者
    task_id: String--parent的ToolTasks内实际创建任务id
    address: String--根据真实父节点与选定节点名确定的完整节点路径
    project_root: PathBuf--Driver传入的完整实际工作目录
    ui_path: PathBuf--Driver指定的实际UI来源
    persistence: P--已连接的持久化组件；plugin使用storage_core::AgentStorage
    reply: Arc<Mutex<Option<oneshot::Sender<Result<AgentHandle, AgentError>>>>>
    impl<P: Send + Sync + 'static> Event for StartAgent<P>

StopAgent：关闭目标Agent所属启动任务的事件
    address: String--目标Agent的完整节点地址
    reply: Arc<Mutex<Option<oneshot::Sender<Result<(), AgentError>>>>>
    impl Event for StopAgent
```

# system

## 函数

```text
-- crate公开
start_agent_system(world: &mut World)
    处理事件：StartAgent<storage_core::AgentStorage>
    接收闭包结果：None
    提交闭包：rollback_created_agent(handle: AgentHandle, reply: Option<AgentReply>, error: AgentError) -> Future<Output = ()>--仅创建后失败补偿
    产出事件：None
    行为：接收StartAgent并从原reply短锁take唯一AgentReply；已被认领则跳过；校验Workspace仍Running、parent仍存活。parent为Workspace时必须等于event.workspace，为Agent时其workspace必须等于event.workspace；其他实体不能作为Agent创建父节点。
        核对parent的ToolTasks中task_id存在、未取消且仍允许绑定子Agent；在本System回复前绑定，事件不可伪造跨Workspace任务归属。
        校验address的父节点对应完整parent，并检查地址未占用；检查event.persistence是已打开的存储，不在本System打开数据库。
        同步建立实体，挂载Address及Agent::new(workspace, parent, task_id, project_root)。
        同步挂载MclQueue::default()、Tools::new()、ToolTasks::new()、inference_core::ActiveInference::default()以及event.persistence。
        在plugin构造AgentHandle；将其关闭回调和完成等待通过tool_core::ToolTaskHandle::bind_child登记到实际创建任务，再回复；parent任务已关闭时立即安排关闭新实体，不返回可用Agent。
        组件全部就绪后依次notify(Installed)和notify(SourcesInstalled { ui_path: Some(event.ui_path) })，对象观察通过注入回调建立。
        首次安装的完整Entity没有旧来源，Installed和首次SourcesInstalled回调必须返回空等待；来源替换仅由既有实体的独立异步受理流程完成，不把等待塞进本System。
        bind_child拒绝或组件安装后失败时，将新句柄通过父任务retain_child_for_cleanup保存为补偿能力，再把句柄、原Sender及错误交给rollback_created_agent；必须等其close实际完成后才用原Sender回复Err。
        创建监督在收到上述错误reply前始终持有节点操作凭证；不是先回复错误再后台清理。接收端丢失时也独立执行同一关闭监督，不丢弃新实例。
        成功通过原reply返回AgentHandle；Sender接收者已丢失也由创建监督关闭刚创建的实体。
        不读取文件、不提交初始化闭包、不启动Driver Lua VM；core事件以泛型P承载存储，agent_core不依赖storage_core。

stop_agent_system(world: &mut World)
    处理事件：StopAgent
    接收闭包结果：None
    提交闭包：Future<Output = ()>
    产出事件：None
    行为：接收StopAgent，按完整节点查找Agent；不存在返回AgentMissing，地址重复返回错误。
        从Agent读取parent、task_id，并核对该parent的实际Address与目标父节点一致；从parent.ToolTasks按task_id取得唯一ToolTaskHandle，不按工具路径、别名或参数猜测。
        将句柄及原reply交给异步闭包调用finish()；该任务的异步子Agent清理调用AgentHandle.close，不再发送StopAgent。
        任务及生命周期绑定的Agent全部实际关闭后回复Ok(())；失败回复AgentError并保留监督。目标及任务均在World同步阶段按完整Entity核对，闭包不借用World。

```

# handler

## 函数

```text
-- crate公开
```

```text
-- 私有
rollback_created_agent(handle: AgentHandle, reply: Option<AgentReply>, error: AgentError) -> Future<Output = ()>
    行为：await handle.close()；reply为Some时成功后才通过原Sender返回原创建错误，失败则返回带清理失败原因的错误，关闭能力保留在父任务待清理项中供显式finish重试。
        成功发送失败意味着接收端已丢失，此时reply=None仍完成关闭，不再尝试回复。
        创建业务失败和补偿收口是两件事，父任务不能在补偿仍执行时宣布创建交接完成。
request_agent_close(world: WorldClosureSender, entity: Entity, state: Arc<Mutex<AgentCloseState>>, runtime: AsyncRuntimeHandle) -> AgentCloseFuture
    行为：同步短锁检查关闭状态；Idle或Failed时登记本次执行并由runtime.spawn启动独立close_agent监督，Running时订阅同一次结果，Closed时直接成功。
        返回的Future只等待结果，不拥有清理执行；等待者取消或丢弃Future不会取消清理。失败后下一次显式close可重新执行尚未完成步骤，不恢复运行受理。
close_agent(world: WorldClosureSender, entity: Entity, state: Arc<Mutex<AgentCloseState>>) -> Future<Output = ()>
    行为：World闭包核对完整Entity；已不存在视为该实例已关闭，不作用于复用index的新实体。
        将Agent.state置Closing、turn清空、ready置Closed并关闭wake；关闭自身MclQueue与ToolTasks的新受理，通知Changed并使旧生成租约失效。
        克隆MclDrain、任务集合、实际推理句柄及存储句柄；向推理及所有自身任务请求取消，等待MCL、推理和任务真实收口。
        仅关闭目标自身创建的子任务，绝不关闭Agent.parent中的创建者任务，避免父任务finish与Agent关闭互相等待。
        调用notify(SourcesClosing)，等待UI、记录和配置来源监控及在途worker全部真实结束；收集错误但不提前丢弃其余等待，所有监控join后再返回聚合错误；工具的诊断引用在子Agent绑定关闭前释放，避免关闭后继续写库。
        用blocking worker同步调用storage_core::close_storage并等待成功；所有克隆随后统一拒绝访问，不能只drop一个Arc。
        World闭包再次核对Entity，despawn后notify(Removed)，然后将AgentCloseState置Closed，通知等待者Ok(())。
        任一步失败置Failed并通知错误；保留Closing实体及尚未释放能力，允许下一次close继续。目录不在此处删除或移动。

```

# types

## 类型

```text
-- 私有，定义于agent_plugin
AgentCloseState：plugin内部关闭监督状态
    phase: AgentClosePhase
    result: watch::Sender<AgentCloseResult>
AgentClosePhase
    Idle | Running | Failed | Closed
AgentCloseResult：Clone
    Pending | Finished(Result<(), AgentError>)
    行为：Running只对应一个独立监督任务；成功永久保存，失败允许显式重试，等待者不持任务清理所有权。

```

# types pub

## 类型

```text
-- 公开，定义于agent_core
AgentReply = oneshot::Sender<Result<AgentHandle, AgentError>>
AgentObservationWait = Pin<Box<dyn Future<Output = Result<(), AgentError>> + Send>>
AgentUpdateKind
    Installed--组件装配完成，初始化状态与集合观察
    Changed--状态或turn实际改变
    SourcesInstalled { ui_path: Option<PathBuf> }--实际持久化组件及可选UI来源已就绪
    SourcesClosing--来源失效、取消监控并返回真实结束等待
    Removed--实体已销毁，移除视图和网络来源

AgentReadyState：Clone + Copy
    Pending | Ready | Closed

AgentState
    Starting--组件已挂载，等待ready；无turn调用可受理
    Running--ready已完成；有turn调用必须与当前有效turn一致，无turn调用不要求伪造回合
    Closing--关闭已开始，拒绝新执行及ready，实际清理完成后销毁实体

AgentWakeSlot
    sender: Option<(u64, oneshot::Sender<()>)>--等待代号与唤醒端
    next_waiter: u64--递增等待代号，取消guard只能清理自己登记的代号
    pending_wake: bool--无人等待时保留一次唤醒
    waiter: Option<u64>--当前等待代号，发送唤醒后仍保留到本等待guard退出
    closed: bool--关闭后拒绝新等待和唤醒
    new() -> Self
        行为：同步创建空槽位，pending_wake及closed为false，waiter=None，next_waiter从1递增。
    wake(&mut self) -> Result<(), AgentError>
        行为：已关闭则返回错误；存在等待者则唤醒，否则记录pending_wake；重复唤醒合并。
    wait(slot: Arc<Mutex<Self>>) -> Future<Output = Result<(), AgentError>>
        行为：已关闭或已有等待者时返回错误；pending_wake为true时消费并立即成功，否则创建oneshot并等待唤醒。
            等待guard只在waiter仍等于自己代号时清空waiter及对应sender；唤醒take sender不清除代号，避免取消旧等待时误删新等待。不持锁跨await。
    close(&mut self)
        行为：置closed，清空pending_wake并释放sender，使正在等待者结束并返回关闭错误。

AgentCloseFuture = Pin<Box<dyn Future<Output = Result<(), AgentError>> + Send>>

AgentHandle：Agent实体的关闭句柄
    entity: Entity--私有，完整Entity，包含generation
    ready: tokio::sync::watch::Receiver<AgentReadyState>--私有，对应Agent的ready槽位
    close: Arc<dyn Fn() -> AgentCloseFuture + Send + Sync>--私有，由agent_plugin注入的关闭能力
    new(entity: Entity, ready: tokio::sync::watch::Receiver<AgentReadyState>, close: impl Fn() -> AgentCloseFuture + Send + Sync + 'static) -> Self
        行为：保存实体身份、ready观察及调用方提供的关闭能力，不创建实体、不挂载组件。
    entity(&self) -> Entity
        行为：返回本句柄对应的实际Entity。
    wait_ready(&self) -> Future<Output = Result<(), AgentError>>
        行为：克隆ready接收端；Ready时成功，Pending等待，Closed或发送端关闭时返回错误；实体Closing但尚未销毁也能结束等待。
    close(&self) -> AgentCloseFuture
        行为：调用已注入能力立即请求受监督关闭，返回结果等待；重复请求共享当前执行，失败允许显式重试，core不读其他组件或发停止事件。
    impl Clone for AgentHandle
        行为：共享同一实际Agent的关闭能力，不创建或复制实体。
```

# error

## 类型

```text
-- 公开，定义于agent_core
AgentError：Agent创建或关闭错误
    message: String--地址校验、实体创建或实际清理的失败说明
    impl Clone + Debug
    impl Display + std::error::Error
```

# 持有关系

```text
World
└── Agent Entity
    ├── Address（address）
    ├── Agent（agent_core）
    │   ├── workspace、parent: Entity
    │   ├── task_id、instance: String
    │   ├── project_root: PathBuf
    │   ├── state: AgentState
    │   ├── turn: Option<String>
    │   ├── ready槽位
    │   └── wake槽位
    ├── MclQueue（mcl_core）
    ├── Tools（tool_resource_core）
    ├── ToolTasks（tool_core）
    ├── ActiveInference（inference_core）
    └── AgentStorage（storage_core）

StartAgent<P>
├── workspace: Entity
├── parent: Entity
├── task_id: String
├── address: String
├── project_root、ui_path: PathBuf
├── persistence: P
└── reply: Arc<Mutex<Option<oneshot::Sender<Result<AgentHandle, AgentError>>>>>

StopAgent
├── address: String
└── reply: Arc<Mutex<Option<oneshot::Sender<Result<(), AgentError>>>>>

调用方
└── AgentHandle（agent_core）
    ├── entity: Entity
    ├── ready观察接收端
    └── 关闭能力（agent_plugin注入）
        └── AgentCloseState
            └── 完整Entity、WorldClosureSender及独立关闭监督状态

AgentUpdateContext
└── 注入回调 → plugin内的版本化公共视图及实际监控归属
```
