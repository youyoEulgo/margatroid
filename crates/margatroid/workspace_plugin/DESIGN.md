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

# WorkspacePlugin

Workspace定义不声明resources或disable_resources。WorkspacePlugin只选择AgentImage、创建Agent并转发运行时消息；资源依赖、默认工具集合和动态工具集合均由Base Lua Driver通过MCL命令管理。

## 统一资源身份约定

```text
Workspace和Workspace中的Agent都是可寻址资源
Workspace使用workspace:local/<workspace-name>:latest的ResourceId，省略tag时为latest
静态Agent使用agent:<workspace-name>/<agent-name>:latest的ResourceId
静态Workspace Agent固定使用latest；WorkspacePlugin不得根据Agent tag创建独立Memory目录，动态Subagent留待后续设计
Workspace查找先按完整ResourceId定位，再得到当前World中的Entity句柄
```

## 类型

公开：
```text
WorkspacePlugin：Workspace运行编排插件，公开结构体--协调Workspace及其AgentInstance的创建、重载和关闭
    agent_images_root: PathBuf--与AgentImageLoaderPlugin相同的镜像库根，私有
    schedule: String--Workspace命令System所属Schedule，私有
    open(agent_images_root: impl Into<PathBuf>) -> Result<Self, WorkspaceError>
        打开插件：公开关联函数，规范化AgentImage根但不扫描或读取镜像
        限制：daemon必须向WorkspacePlugin和AgentImageLoaderPlugin传入同一个镜像库根
    with_schedule(mut self, schedule: impl Into<String>) -> Self
        指定阶段：公开方法，替换默认RuntimePlugin::UPDATE并返回自身
    impl Plugin for WorkspacePlugin
        Plugin：公开trait实现
        build(self, app: &mut App)
            构建插件：安装WorkspaceRegistry并挂载Workspace命令System
            行为：
                确认RuntimePlugin、ResourceIdPlugin、AgentImageLoaderPlugin、InferencePlugin、ToolPlugin、MclPlugin、AgentPlugin和MemoryPlugin已安装；缺失任一依赖时终止配置
                确认schedule存在且WorkspacePlugin尚未安装
                插入WorkspaceRegistry
                挂载begin_workspace_command_system
                挂载route_agent_message_system
                挂载route_agent_turn_abort_system
                挂载route_agent_workflow_system
                挂载route_agent_visibility_system
                挂载collect_agent_image_system
                挂载collect_agent_create_result_system
                挂载collect_agent_initialization_system

StartWorkspace：Workspace启动事件，公开事件--从Compose编译后的定义创建一组AgentInstance
    id: String--调用方生成的请求ID
    definition: WorkspaceDefinition--Compose产生的完整静态定义
    impl Event for StartWorkspace
        Event：公开trait实现

StartWorkspaceResult：Workspace Entity创建结果，公开事件--不等待Agent Entity或资源注入
    id: String--原请求ID
    result: Result<Entity, WorkspaceError>--成功时Workspace已经可查询；只有Ready Agent可以接收成员消息
    impl Event for StartWorkspaceResult
        Event：公开trait实现

ReloadWorkspace：Workspace重载事件，公开事件--关闭当前实例组并按新定义重新创建
    id: String--调用方生成的请求ID
    workspace: Entity--当前已就绪Workspace Entity
    definition: WorkspaceDefinition--Compose重新编译的最新定义
    impl Event for ReloadWorkspace
        Event：公开trait实现

ReloadWorkspaceResult：Workspace Entity重建结果，公开事件--不等待新Workspace的Agent Entity或资源注入
    id: String--原请求ID
    previous: Entity--被关闭的原Workspace Entity
    result: Result<Entity, WorkspaceError>--成功时为重建后的已就绪Workspace Entity
    impl Event for ReloadWorkspaceResult
        Event：公开trait实现

StopWorkspace：Workspace关闭事件，公开事件--释放Workspace拥有的全部AgentInstance
    id: String--调用方生成的请求ID
    workspace: Entity--需要关闭的已就绪Workspace Entity
    impl Event for StopWorkspace
        Event：公开trait实现

StopWorkspaceByReference：按逻辑引用关闭Workspace事件，公开事件--供跨进程DTO在没有Entity时请求关闭
    id: String--请求ID
    workspace: WorkspaceReference--Workspace名称和项目根
    impl Event for StopWorkspaceByReference
        Event：公开trait实现

StopWorkspaceResult：Workspace关闭结果，公开事件--报告Workspace和Agent释放结果
    id: String--原请求ID
    workspace: Entity--原Workspace Entity
    result: Result<(), WorkspaceError>--成功表示Workspace和其AgentInstance已经释放
    impl Event for StopWorkspaceResult
        Event：公开trait实现

StopWorkspaceByReferenceResult：逻辑引用关闭结果，公开事件--供DTO层生成停止回执
    id: String--请求ID
    workspace: WorkspaceReference--已请求关闭的Workspace逻辑引用
    result: Result<(), WorkspaceError>--关闭结果
    impl Event for StopWorkspaceByReferenceResult
        Event：公开trait实现

Workspace：Workspace聚合组件，公开组件--Workspace Entity唯一的领域数据组件
    definition: Arc<WorkspaceDefinition>--本次启动使用的编译后定义，私有
    project_root: Arc<PathBuf>--规范化绝对项目根，私有
    manager_name: String--Workspace定义中的默认入口Agent逻辑名称，私有
    agents: BTreeMap<String, Entity>--Agent逻辑名称到Entity的稳定映射，私有
    states: BTreeMap<String, WorkspaceAgentState>--每个定义成员的当前创建状态，私有
    definition(&self) -> &WorkspaceDefinition
        取得定义：公开方法，返回只读配置引用
    project_root(&self) -> &Path
        取得项目根：公开方法，返回规范化项目根
    manager(&self) -> Option<Entity>
        取得默认入口：公开方法
    agent(&self, name: &str) -> Option<Entity>
        取得Agent：公开方法
    state(&self, name: &str) -> Option<&WorkspaceAgentState>
        查询成员状态：公开方法
    states(&self) -> impl Iterator<Item = (&str, &WorkspaceAgentState)> + '_
        遍历成员状态：公开方法
    iter(&self) -> impl Iterator<Item = (&str, Entity)> + '_
        遍历已创建Agent：公开方法
    impl Component for Workspace
        Component：公开trait实现

Workspace Entity硬约束：Entity必须且只能挂载ResourceId和Workspace两个组件。
    ResourceId是统一身份组件；Workspace保存身份之外的全部Workspace领域状态。
    WorkspaceModelRoutes由InferencePlugin保存于自身Resource索引，不挂载到Workspace Entity。

WorkspaceAgentState：Workspace成员创建状态，公开枚举
    Creating--已经发布Workspace成功通知，Agent仍处于镜像加载、材料准备或Entity创建阶段
    Ready { agent: Entity }--Agent Entity与外部运行组件均已建立；资源可见性随后独立热插拔
    Failed { error: WorkspaceError }--当前成员创建失败；不影响Workspace及其他Agent

WorkspaceErrorKind：Workspace错误分类，公开枚举
    InvalidRequest
    InvalidDefinition
    InvalidProjectRoot
    InvalidAgentImagesRoot
    DuplicateWorkspace
    WorkspaceNotAlive
    WorkspaceNotReady
    WorkspaceMismatch
    AgentImageLoadFailed
    AgentImageComponentsMissing
    InferenceSetupFailed
    MemorySetupFailed
    ResourceSetupFailed
    AgentCreateFailed
    impl Clone + Copy + PartialEq + Eq for WorkspaceErrorKind
        值语义：公开trait实现

WorkspaceError：Workspace命令错误，公开结构体--提供不暴露绝对路径或资源正文的稳定描述
    kind: WorkspaceErrorKind--错误分类，私有
    message: String--不包含Soul、Memory消息、Skill正文或工具参数的有界描述，私有
    kind(&self) -> WorkspaceErrorKind
        取得分类：公开方法，返回错误分类
    message(&self) -> &str
        取得描述：公开方法，返回有界错误描述
    new(kind: WorkspaceErrorKind, message: impl Into<String>) -> Self
        构造错误：私有关联函数，保存分类和有界描述
    impl Clone for WorkspaceError
        Clone：公开trait实现
    impl fmt::Display for WorkspaceError
        Display：公开trait实现，只输出稳定分类和描述
    impl std::error::Error for WorkspaceError
        Error：公开trait实现

WorldWorkspaceExt：World Workspace扩展，公开trait--发送Workspace命令并查询运行时关系
    start_workspace(&self, id: impl Into<String>, definition: WorkspaceDefinition)
        启动Workspace：公开方法，发送StartWorkspace并唤醒Runtime
    reload_workspace(
        &self,
        id: impl Into<String>,
        workspace: Entity,
        definition: WorkspaceDefinition,
    )
        重载Workspace：公开方法，发送ReloadWorkspace并唤醒Runtime
    stop_workspace(&self, id: impl Into<String>, workspace: Entity)
        关闭Workspace：公开方法，发送StopWorkspace并唤醒Runtime
    workspaces(&self) -> Vec<Entity>
        列出Workspace：公开方法，返回当前已登记且仍存活的全部Workspace Entity
    workspace_by_id(&self, id: &ResourceId) -> Option<Entity>
        按资源ID查询Workspace：公开方法，按完整ResourceId返回已登记且仍存活的Workspace Entity
    workspace(&self, project_root: &Path, name: &str) -> Option<Entity>
        兼容查询Workspace：公开方法，把名称规范化为workspace:local/<name>:latest，并同时校验项目根
    workspace_agent(&self, workspace: Entity, name: &str) -> Option<Entity>
        查询Agent：公开方法，从WorkspaceAgents按逻辑名称返回存活Agent
    workspace_manager(&self, workspace: Entity) -> Option<Entity>
        查询默认入口：公开方法，返回存活manager Agent
    workspace_of(&self, agent: Entity) -> Option<Entity>
        反查Workspace：公开方法，从AgentWorkspaceId返回存活Workspace
    impl WorldWorkspaceExt for World
        WorldWorkspaceExt for World：公开trait实现，命令通过Runtime事件队列发送
```

crate公开：
```text
WorkspaceRegistry：Workspace注册与Agent创建监控状态，crate公开Resource
    agent_images_root: Arc<PathBuf>--AgentImage版本目录推导根
    workspaces: Vec<Entity>--Workspace实体生命周期集合，不复制ResourceId索引
    initializations: HashMap<Entity, WorkspaceAgentInitialization>--Workspace Entity到尚未结束的成员创建材料
    image_requests: HashMap<String, (Entity, String)>--镜像子请求ID到Workspace Entity和Agent名称
    agent_requests: HashMap<String, (Entity, String)>--Agent创建子请求ID到Workspace Entity和Agent名称
    impl Resource for WorkspaceRegistry
        Resource：crate公开trait实现
```

私有：
```text
WorkspaceKey：Workspace注册键，私有结构体--由规范化project_root和name组成
    project_root: PathBuf--规范化项目根
    name: String--Workspace逻辑名称
    from_definition(definition: &WorkspaceDefinition) -> Self
        从定义构造：私有关联函数，克隆规范化项目根和Workspace名称

WorkspaceAgentInitialization：Workspace成员创建监控，私有结构体--不决定Workspace是否成立
    remaining: BTreeSet<String>--尚未进入Ready或Failed的Agent逻辑名称
    prepared: BTreeMap<String, PreparedWorkspaceAgent>--已经完成材料准备且等待AgentCreateResult的成员
    agents_by_entity: BTreeMap<Entity, String>--已经收到AgentCreateResult、等待AgentInitializationCompleted的Entity到成员名称映射

PreparedWorkspaceAgent：单Agent实例材料，私有结构体--创建Agent Entity前收集完整依赖
    name: String--Agent逻辑名称
    agent_id: ResourceId--Workspace生成的稳定Agent ID，例如agent:<workspace>/<name>:latest
    base_lua: LuaProgram--从Agent镜像取得的内禀base.lua程序
    memory: AgentMemoryHandle--MemoryPlugin已经打开的存储句柄，由AgentCreateRequest在启动Base Lua前写入Agent
    token_usage: TokenUsage--MemoryPlugin从历史Assistant行恢复的累计Token
    inference_snapshot: AgentInferenceSnapshot--InferencePlugin构造的实例推理快照，包含标准化后的context_window_tokens
    tool_environment: AgentToolEnvironment--项目根与镜像版本根
```

## 函数

私有：
```text
route_agent_message_system(world: &mut World)
    路由成员消息：私有System，读取RouteAgentMessage并把逻辑Workspace和Agent名称解析为Entity
    行为：
        按WorkspaceReference查询已经建立的Workspace Entity
        agent为空时使用WorkspaceDefinition.manager
        目标成员仍在Creating或已经Failed时路由失败
        只接受Message::User
        直接保留Message::User中的content并发送AgentMessage { id, agent, message, usage: None }
        路由失败时记录警告且不构造AgentMessage

route_agent_turn_abort_system(world: &mut World)
    路由成员轮次中止：私有System，读取RouteAgentTurnAbort并把逻辑Workspace和Agent名称解析为Entity
    行为：agent为空时使用WorkspaceDefinition.manager；成功时创建一次性回执并发送AgentControl { id, agent, control=AbortTurn, reply }

route_agent_visibility_system(world: &mut World)
    路由默认资源可见性命令：私有System，读取RouteAgentVisibility并解析Workspace和Agent Entity
    行为：agent为空时使用manager；创建一次性回执，把Inject或Remove转换为AgentControlKind::InjectVisibility或RemoveVisibility并发送AgentControl
    边界：WorkspacePlugin只解析逻辑身份，不读取或修改Agent可见性

route_agent_workflow_system(world: &mut World)
    路由Workflow热插拔：私有System，读取RouteAgentWorkflowAttach与RouteAgentWorkflowDetach
    行为：
        复用WorkspaceReference和可选Agent资源ID解析Ready成员，agent为空时使用manager
        Attach从AgentToolEnvironment取得project_root与image_root并发送AttachWorkflowMcl
        Detach验证instance_id并发送DetachWorkflowMcl
        WorkspacePlugin不读取MCL源码、不编译、不修改Agent.mcl

begin_workspace_command_system(world: &mut World)
    开始命令：私有System，读取StartWorkspace、ReloadWorkspace、StopWorkspace和StopWorkspaceByReference
    行为：
        收集本次全部Workspace命令并结束EventReader借用
        StartWorkspace调用create_workspace_entity；成功时立即发布StartWorkspaceResult::Ok，再异步创建成员
        ReloadWorkspace验证旧Workspace已登记、最新定义合法且WorkspaceKey与旧身份一致
        ReloadWorkspace验证成功后关闭旧实例并调用create_workspace_entity；成功时立即发布ReloadWorkspaceResult::Ok
        StopWorkspace调用stop_workspace_inner并立即发送StopWorkspaceResult
        StopWorkspaceByReference先查询Entity，再调用stop_workspace_inner并发送StopWorkspaceByReferenceResult
        只有Workspace定义、身份、项目路由或Workspace Entity创建失败时发布Workspace命令错误
        Agent镜像、Memory、Inference、Entity或资源注册失败不改变已经发布的Workspace成功结果

create_workspace_entity(
    world: &mut World,
    definition: WorkspaceDefinition,
) -> Result<Entity, WorkspaceError>
    创建Workspace Entity：私有函数，成功返回后Workspace已经可寻址，但成员可以仍在Creating或全部Failed
    行为：
        验证Workspace名称、绝对project_root、非空agents和唯一Agent名称
        验证manager对应一个Agent定义
        Agent名称必须能够安全用于默认Memory目录段
        memory_path存在时必须是Compose解析后的绝对路径
        相同WorkspaceKey已经登记时返回DuplicateWorkspace
        创建Workspace Entity并插入ResourceId和完整Workspace
        Workspace为全部定义成员写入Creating，agents为空，manager_name来自definition.manager
        调用WorldInferenceExt::load_workspace_model_routes(workspace, project_root)
        项目级模型路由加载失败时despawn Workspace Entity并返回错误
        立即写入WorkspaceRegistry.workspaces；从此Workspace查询、停止和状态同步均可见
        创建WorkspaceAgentInitialization.remaining并为每个Agent发送LoadAgentImage
        保存initializations和所有image_requests路由
        返回Workspace Entity；不等待任何Agent结果

collect_agent_image_system(world: &mut World)
    收集镜像：私有System，读取LoadAgentImageResult并逐个推进成员创建
    行为：
        根据结果id从image_requests定位Workspace Entity与Agent名称
        Workspace已经停止或路由失效时忽略迟到响应
        失败时调用mark_agent_failed，只将该Agent状态改为Failed
        成功时调用prepare_workspace_agent
        prepare_workspace_agent失败时调用mark_agent_failed
        成功准备该Agent后生成独立创建子请求ID，保存prepared和agent_requests并发送携带累计Token的AgentCreateRequest
        不等待其他Agent，不因单个成员失败回滚已经创建的成员

prepare_workspace_agent(world: &mut World, workspace: Entity, name: &str, image: Entity) -> Result<PreparedWorkspaceAgent, WorkspaceError>
    准备单个实例：私有函数，为一个Agent构造创建材料
    行为：
        从Workspace.definition按name取得WorkspaceAgentDefinition
        读取AgentImage聚合字段和ResourceId组件
        调用WorldInferenceExt::build_agent_inference_snapshot构造AgentInferenceSnapshot
        根据agent_images_root和type=image的ResourceId构造镜像版本根
        使用definition.project_root和镜像版本根构造AgentToolEnvironment
        memory_path为空时生成<project>/.margatroid/workspaces/<workspace>/memory/<agent>/memory.sql
        调用AgentMemory::open取得AgentMemoryHandle和RealtimeContext；RealtimeContext.ordered_messages不注入Agent或MCL
        收集Base Driver、memory和RealtimeContext.token_usage；实时消息只允许Base Lua随后通过realtime_load读取
        构造并返回PreparedWorkspaceAgent
        任一步失败时释放该Agent已经打开的AgentMemory并返回错误，不影响其他成员

collect_agent_create_result_system(world: &mut World)
    收集Agent Entity创建结果：私有System，消费AgentCreateResult并补齐外部运行组件
    行为：
        根据AgentCreateResult.id从agent_requests定位Workspace Entity与Agent名称
        校验agent_id等于PreparedWorkspaceAgent.agent_id
        Err时调用mark_agent_failed，不影响Workspace或其他成员
        Ok时从initializations.prepared取出对应PreparedWorkspaceAgent
        验证Agent Entity存活且AgentWorkspaceId指向当前Workspace
        验证Agent.memory等于AgentCreateRequest交付的PreparedWorkspaceAgent.memory；存储在Base Lua启动前已经可读
        插入PreparedWorkspaceAgent.inference_snapshot
        插入PreparedWorkspaceAgent.tool_environment
        AgentResourceMap已经在唯一Agent组件的resources字段中初始化；这里只验证它存在
        全部绑定成功后把Workspace对应成员保留为Creating并写入agents索引；AgentCreateResult成功只表示Entity已创建，不表示Base Lua初始化完成
        AgentPlugin已在Agent Entity创建成功时发送恢复默认可见性通知；WorkspacePlugin不介入资源注册或可见性修改
        绑定失败时despawn当前Agent并调用mark_agent_failed
        把Entity到成员名称写入initializations.agents_by_entity；只有收到AgentInitializationCompleted后才进入Ready并从remaining删除；AgentCreate失败直接进入Failed

attach_prepared_agent(world: &mut World, workspace: Entity, name: &str, agent: Entity, prepared: PreparedWorkspaceAgent) -> Result<(), WorkspaceError>
    绑定实例材料：私有函数，验证Agent归属并绑定Memory、Inference和AgentToolEnvironment
    行为：
        验证Entity同时挂载ResourceId和Agent，且Agent包含resources和mcl字段；缺失时返回ResourceSetupFailed
        绑定PreparedWorkspaceAgent中的Memory、InferenceSnapshot和AgentToolEnvironment
        不修改Agent.mcl Block，不发送资源注册请求

collect_agent_initialization_system(world: &mut World)
    收集Base Lua初始化：私有System，读取AgentInitializationCompleted和LuaRuntimeTaskFinished
    行为：
        AgentInitializationCompleted按Entity查找WorkspaceAgentInitialization.agents_by_entity
        仅当Agent.lifecycle已经Running且vm_id匹配时把成员改为Ready，并从remaining删除
        LuaRuntimeTaskFinished在Agent仍处于Creating时把成员改为Failed；运行后的停止只更新既有Agent状态，不重复改变Workspace初始化结果
        remaining为空时清理initializations；迟到或未知Entity事件幂等忽略

mark_agent_failed(world: &mut World, workspace: Entity, name: &str, error: WorkspaceError)
    标记成员失败：私有函数，不改变Workspace成功状态
    行为：
        释放该成员尚未绑定的PreparedWorkspaceAgent和AgentMemory
        把WorkspaceAgents.states[name]改为Failed并确保agents索引中不存在name
        从WorkspaceAgentInitialization.remaining删除name
        记录并向外部投影Agent创建失败日志；不发送StartWorkspaceResult::Err
        remaining为空时清理该Workspace的initializations

stop_workspace_inner(world: &mut World, workspace: Entity) -> Result<(), WorkspaceError>
    关闭Workspace：私有函数，停止查询并释放Workspace拥有的AgentInstance
    行为：
        验证workspace存活、包含WorkspaceIdentity和WorkspaceAgents且已登记在WorkspaceRegistry.ready
        从WorkspaceRegistry.ready移除对应WorkspaceKey
        清理该Workspace尚未完成的image_requests、agent_requests和initializations；迟到响应随后被忽略
        依次despawn全部已创建成功的Agent Entity，使AgentMemory连接随组件释放
        despawn Workspace Entity
        不删除memory.sql、项目文件或AgentImage Entity

validate_request_id(world: &World, id: &str) -> Result<(), WorkspaceError>
    验证请求ID：私有函数，拒绝空ID；Workspace命令结果在当前事件层恰好发送一次

validate_definition(definition: WorkspaceDefinition) -> Result<WorkspaceDefinition, WorkspaceError>
    验证定义：私有函数，规范化项目根和Memory路径，验证Agent唯一性与manager引用

validate_agent_definition(agent: &WorkspaceAgentDefinition, workspace_name: &str) -> Result<(), WorkspaceError>
    验证Agent定义：私有函数，检查逻辑名称、Agent资源ID的type/scope/name一致性和AgentImage资源类型；保留完整实例tag

normalize_project_root(path: PathBuf) -> Result<PathBuf, WorkspaceError>
    规范化项目根：私有函数，要求绝对路径且不包含父级跳转
    行为：拒绝相对路径、Root/Prefix之外的Parent组件和无法规范化的路径

normalize_agent_images_root(path: PathBuf) -> Result<PathBuf, WorkspaceError>
    规范化镜像根：私有函数，要求绝对路径且不包含父级跳转

normalize_memory_path(path: PathBuf) -> Result<PathBuf, WorkspaceError>
    规范化Memory路径：私有函数，要求绝对路径且不包含父级跳转

normalize_absolute_path(path: PathBuf) -> Option<PathBuf>
    规范化绝对路径：私有函数，移除CurDir并拒绝相对路径和ParentDir

validate_logical_name(value: &str) -> Result<(), WorkspaceError>
    验证逻辑名称：私有函数，要求非空、长度有界且可安全作为单个目录段
    行为：拒绝控制字符、路径分隔符、.、..和其他不能作为单目录段的值

image_root(root: &Path, reference: &ResourceId) -> PathBuf
    构造镜像版本根：私有函数，要求type=image，依次拼接scope、name和tag

default_memory_path(project_root: &Path, agent: &ResourceId) -> PathBuf
    构造默认Memory路径：私有函数，静态Agent统一使用项目内Agent memory.sql路径；不按tag创建目录，Subagent路径策略留待后续设计

agent_image_components_missing() -> WorkspaceError
    构造镜像组件错误：私有函数，返回稳定AgentImageComponentsMissing错误

ready_workspace_key(world: &World, workspace: Entity) -> Result<WorkspaceKey, WorkspaceError>
    取得就绪键：私有函数，验证Entity、Identity、Agents和ready索引一致

workspace_by_reference(world: &World, reference: &WorkspaceReference) -> Option<Entity>
    按引用查询Workspace：私有函数，按完整ResourceId查询，并复核规范化项目根与Workspace名称

is_registered_workspace(world: &World, workspace: Entity) -> bool
    检查已登记Workspace：私有函数，复用ready_workspace_key

cleanup_orphan_agent(world: &mut World, agent: Entity)
    清理孤立Agent：私有函数，仅在Agent的Workspace已经失活时释放Agent
```

## 逻辑

```text
安装：
    AgentImageLoaderPlugin
        -> InferencePlugin
        -> ToolPlugin
        -> BuiltinToolPlugin
        -> MemoryPlugin
        -> AgentPlugin
        -> WorkspacePlugin::open(agent_images_root)
    WorkspacePlugin只协调已有Plugin，不替它们重新实现加载、验证或执行
    WorkspacePlugin不解析YAML，不识别资源Provider，不读取资源正文，不执行推理、工具或Agent消息循环

输入与收集边界：
    Compose
        -> 构造WorkspaceDefinition和WorkspaceAgentDefinition
        -> 不创建Entity，不加载AgentImage，不打开Memory
    AgentImageLoaderPlugin
        -> AgentImage Entity、Base Driver、中立模型配置和镜像版本根
    InferencePlugin
        -> Workspace项目级模型路由和AgentInferenceSnapshot
    ToolPlugin
        -> AgentToolEnvironment类型和AgentResourceMap行为
        -> AgentResourceMap由AgentPlugin创建Agent时初始化在Agent.resources
    MemoryPlugin
        -> AgentMemory
        -> 打开数据库时恢复的RealtimeContext { ordered_messages, token_usage }
    WorkspacePlugin自身
        -> Workspace归属、逻辑名称和Agent Entity索引
        -> 把Base Driver、AgentToolEnvironment和AgentMemoryHandle交给AgentCreateRequest
        -> Workspace Entity成立后立即发布成功通知，并独立监控每个Agent的Creating、Ready或Failed
    Base Driver初始化时执行IMPORT并用tool_default覆盖tool_dynamic
        -> MclPlugin发送AgentResourceRegisterRequest，由BuiltinToolPlugin选择具体Provider验证资源并构造候选ResourceMapEntry
        -> MclPlugin在IMPORT事务中调用ToolPlugin接口写入AgentResourceMap；resource_name使用MCL alias或完整ResourceId
        -> MclPlugin通过原MclCommandReply把IMPORT成功或失败返回Base Lua调用点
        -> 工具正文和运行时内容不缓存；每次调用时由对应Plugin重新读取

workspace up：
    Compose生成WorkspaceDefinition
        -> world.start_workspace(id, definition)
        -> begin_workspace_command_system校验并创建Workspace Entity
        -> WorldInferenceExt::load_workspace_model_routes(workspace, project_root)
        -> 挂载WorkspaceAgents { Creating * N }并登记WorkspaceRegistry.ready
        -> 立即发布StartWorkspaceResult::Ok
        -> 发送LoadAgentImage * N
        -> 每个Agent独立处理LoadAgentImageResult
           ├── 失败 -> WorkspaceAgentState::Failed
           └── 成功 -> 准备该Agent的Inference、Tool、Memory材料
                      ├── 失败 -> WorkspaceAgentState::Failed
                      └── 成功 -> AgentCreateRequest
                                 -> AgentCreateResult
                                    ├── 失败 -> WorkspaceAgentState::Failed
                                    └── 成功 -> Workspace绑定外部运行组件
                                               -> Creating
                                               -> AgentInitializationCompleted
                                               -> WorkspaceAgentState::Ready
        AgentPlugin创建成功后启动Base Driver；只有Base Lua第一次成功登记start才发布AgentInitializationCompleted
            -> 资源逐项注入成功或失败通知
        -> 任意Agent失败均不撤销Workspace，不改变已经发布的StartWorkspaceResult::Ok

项目级模型路由：
    WorkspacePlugin传入已经规范化的project_root
        -> InferencePlugin读取<project_root>/.margatroid/models.toml
        -> 存在时编译并写入InferencePlugin自己的WorkspaceModelRoutesRegistry Resource
        -> 不存在时从该Resource索引移除旧路由并返回0
    每次InferenceRequestEvent：
        -> WorkspaceModelRoutes[AgentInferenceSnapshot.model]
        -> 未命中时GlobalModelRoutes[model]
        -> 都未命中时InferencePlugin发送AgentFailure

运行时查询：
    ServerPlugin取得Workspace Entity
        -> WorldWorkspaceExt::workspace_manager(workspace)
        -> 取得默认入口Agent
    Agent或其他Plugin持有Agent Entity
        -> WorldWorkspaceExt::workspace_of(agent)
        -> 读取AgentWorkspaceId反查Workspace
        -> 读取WorkspaceIdentity取得项目根
        -> 读取各业务Plugin挂载的typed Component取得实际配置
    WorkspacePlugin不提供String到Any的通用属性表

workspace reload：
    Compose重新读取Workspace文件并生成新WorkspaceDefinition
        -> world.reload_workspace(id, old_workspace, definition)
        -> 验证旧Workspace已登记且WorkspaceKey不变
        -> stop_workspace_inner关闭旧Workspace和全部Agent
        -> 按workspace up流程创建新Workspace Entity并立即发布ReloadWorkspaceResult
        -> 新Workspace Entity创建失败时发布Err，旧Workspace不恢复
        -> 新Workspace的单个或全部Agent失败不使Reload结果失败
        -> 当前阶段不同时运行新旧实例，不实现无停机热切换

workspace down：
    world.stop_workspace(id, workspace)
        -> 验证Workspace已登记且包含Workspace
        -> 从WorkspaceRegistry.workspaces移除实体
        -> despawn全部AgentInstance
        -> despawn Workspace Entity
        -> 保留memory.sql、项目文件和AgentImage Entity

所有权与边界：
    WorkspacePlugin拥有Workspace Entity和由它启动的AgentInstance；Workspace Entity创建成功后立即进入ready，允许manager缺失或全部Agent失败
    AgentImageLoaderPlugin拥有AgentImage Entity
    MemoryPlugin拥有SQLite格式与AgentMemory组件行为
    InferencePlugin拥有WorkspaceModelRoutesRegistry Resource和AgentInferenceSnapshot；不向Workspace Entity挂载组件
    ToolPlugin拥有AgentToolEnvironment类型，WorkspacePlugin只负责构造实例值
    AgentPlugin分别挂载ResourceId和唯一Agent组件；Agent组件统一包含MCL、资源、推理、工具、存储和Lua运行状态
    AgentPlugin拥有资源注册协调、飞行中注册关联和逐项可见性修改；WorkspacePlugin不保存或等待资源注册状态
    各业务Plugin拥有自己挂在Workspace或Agent上的typed Component语义
    WorkspacePlugin只拥有编排决策，不复制其他Plugin的运行时状态
```

## 持有关系

```text
App
└── World
    ├── WorkspaceRegistry Resource
    │   ├── agent_images_root
    │   ├── workspaces: Vec<Entity>
    │   └── initializations: HashMap<Entity, WorkspaceAgentInitialization>
    ├── Workspace Entity
    │   ├── ResourceId
    │   └── Workspace
    │   └── AgentInstance Entity * N
    │       ├── ResourceId--统一身份组件
    │       └── Agent--共享数据组件，由AgentPlugin挂载；组件存在本身表示该Entity是Agent
    └── AgentImage Entity * N
        ├── ResourceId
        └── AgentImage--AgentImageLoaderPlugin拥有

启动事件链：
StartWorkspace
    -> Workspace Entity + Workspace { states: Creating * N }
    -> StartWorkspaceResult::Ok
    -> LoadAgentImage * N
       -> LoadAgentImageResult * N
          -> 每个成员独立进入Failed或PreparedWorkspaceAgent
             -> AgentCreateRequest
                -> AgentCreateResult
                   -> Failed或Workspace绑定Memory、InferenceSnapshot和ToolEnvironment
                      -> Creating
                      -> AgentInitializationCompleted
                      -> Ready
        AgentPlugin创建成功后启动Base Driver；初始化失败通过LuaRuntimeTaskFinished进入Failed
            -> AgentVisibleResourceInjected | AgentVisibleResourceInjectionFailed
```
