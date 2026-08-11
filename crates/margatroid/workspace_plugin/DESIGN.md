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
                确认RuntimePlugin、AgentImageLoaderPlugin、InferencePlugin、ToolPlugin、AgentPlugin和MemoryPlugin已安装；缺失任一依赖时终止配置
                确认schedule存在且WorkspacePlugin尚未安装
                插入WorkspaceRegistry
                挂载begin_workspace_command_system
                挂载route_agent_message_system
                挂载collect_agent_image_system
                挂载collect_agent_created_system

StartWorkspace：Workspace启动事件，公开事件--从Compose编译后的定义创建一组AgentInstance
    id: String--调用方生成的请求ID
    definition: WorkspaceDefinition--Compose产生的完整静态定义
    impl Event for StartWorkspace
        Event：公开trait实现

StartWorkspaceResult：Workspace启动结果，公开事件--返回已就绪Workspace Entity或稳定错误
    id: String--原请求ID
    result: Result<Entity, WorkspaceError>--成功时为可接收请求的Workspace Entity
    impl Event for StartWorkspaceResult
        Event：公开trait实现

ReloadWorkspace：Workspace重载事件，公开事件--关闭当前实例组并按新定义重新创建
    id: String--调用方生成的请求ID
    workspace: Entity--当前已就绪Workspace Entity
    definition: WorkspaceDefinition--Compose重新编译的最新定义
    impl Event for ReloadWorkspace
        Event：公开trait实现

ReloadWorkspaceResult：Workspace重载结果，公开事件--返回新建Workspace或稳定错误
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

WorkspaceIdentity：Workspace身份，公开组件--保存稳定逻辑名称与规范化项目根
    name: Arc<str>--项目内Workspace逻辑名称，私有
    project_root: Arc<PathBuf>--规范化绝对项目根，私有
    name(&self) -> &str
        取得名称：公开方法，返回Workspace逻辑名称
    project_root(&self) -> &Path
        取得项目根：公开方法，返回规范化项目根
    impl Component for WorkspaceIdentity
        Component：公开trait实现

WorkspaceConfiguration：Workspace配置快照，公开组件--保存本次启动使用的编译后定义
    definition: Arc<WorkspaceDefinition>--不包含运行时Entity的静态定义，私有
    definition(&self) -> &WorkspaceDefinition
        取得定义：公开方法，返回只读配置引用
    impl Component for WorkspaceConfiguration
        Component：公开trait实现

WorkspaceAgents：Workspace Agent索引，公开组件--只在全部Agent准备成功后挂载
    manager: Entity--默认入口AgentInstance，私有
    agents: BTreeMap<String, Entity>--Agent逻辑名称到Entity的稳定映射，私有
    manager(&self) -> Entity
        取得默认入口：公开方法，返回manager Agent Entity
    agent(&self, name: &str) -> Option<Entity>
        取得Agent：公开方法，按逻辑名称查询Agent Entity
    iter(&self) -> impl Iterator<Item = (&str, Entity)> + '_
        遍历Agent：公开方法，按逻辑名称顺序返回全部Agent
    impl Component for WorkspaceAgents
        Component：公开trait实现

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
    workspace(&self, project_root: &Path, name: &str) -> Option<Entity>
        查询Workspace：公开方法，只返回已登记且仍存活的Workspace Entity
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
WorkspaceRegistry：Workspace注册与操作状态，crate公开Resource--保存已就绪索引和未完成操作
    agent_images_root: Arc<PathBuf>--AgentImage版本目录推导根
    ready: HashMap<WorkspaceKey, Entity>--规范化项目根与Workspace名称到已就绪Workspace Entity
    pending: HashMap<String, PendingWorkspace>--Workspace请求ID到未完成操作
    image_requests: HashMap<String, (String, String)>--镜像子请求ID到Workspace请求ID和Agent名称
    agent_requests: HashMap<String, (String, String)>--Agent创建子请求ID到Workspace请求ID和Agent名称
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

PendingWorkspaceKind：未完成操作类型，私有枚举
    Start--启动操作
    Reload { previous: Entity }--重载操作及已关闭的旧Workspace

PendingWorkspace：未完成Workspace操作，私有结构体--暂存跨多个事件阶段的启动材料
    kind: PendingWorkspaceKind--决定最终发布Start或Reload结果
    workspace: Entity--本次操作暂存的Workspace Entity
    definition: WorkspaceDefinition--已复核定义
    images: BTreeMap<String, Result<Entity, WorkspaceError>>--每个Agent的镜像收集结果
    prepared: BTreeMap<String, PreparedWorkspaceAgent>--全部镜像成功后构造的实例材料
    agents: BTreeMap<String, Entity>--已经收到回执并完成其他Plugin组件绑定的Agent

PreparedWorkspaceAgent：单Agent实例材料，私有结构体--创建Agent Entity前收集完整依赖
    name: String--Agent逻辑名称
    agent_id: String--Workspace生成的稳定Agent ID，例如<workspace>.<name><index>
    system_prompt: String--从AgentImage Soul取得的系统提示词
    messages: Vec<Message>--MemoryPlugin恢复的长期对话上下文
    tool_context: Vec<Message>--MemoryPlugin恢复的当前轮工具上下文
    default_visibility: BTreeSet<ResourceRef>--镜像默认值与Workspace参数合并后的资源集合，交给AgentPlugin构造只读组件
    inference_snapshot: AgentInferenceSnapshot--InferencePlugin构造的实例推理快照
    tool_environment: AgentToolEnvironment--项目根与镜像版本根
    memory: AgentMemory--已经打开但尚未绑定Entity的SQLite连接
```

## 函数

私有：
```text
route_agent_message_system(world: &mut World)
    路由成员消息：私有System，读取RouteAgentMessage并把逻辑Workspace和Agent名称解析为Entity
    行为：
        按WorkspaceReference查询已就绪Workspace
        agent为空时使用WorkspaceDefinition.manager
        只接受Message::User
        直接保留Message::User中的content和tool_calls并发送AgentMessage { id, agent, message }
        路由失败时记录警告且不构造AgentMessage

begin_workspace_command_system(world: &mut World)
    开始命令：私有System，读取StartWorkspace、ReloadWorkspace、StopWorkspace和StopWorkspaceByReference
    行为：
        收集本次全部Workspace命令并结束EventReader借用
        StartWorkspace调用begin_workspace_start
        ReloadWorkspace验证旧Workspace已登记、最新定义合法且WorkspaceKey与旧身份一致
        ReloadWorkspace验证成功后调用stop_workspace_inner关闭旧实例，再以Reload类型调用begin_workspace_start
        StopWorkspace调用stop_workspace_inner并立即发送StopWorkspaceResult
        StopWorkspaceByReference先查询Entity，再调用stop_workspace_inner并发送StopWorkspaceByReferenceResult
        每个命令失败时发布对应的稳定错误结果

begin_workspace_reload(world: &mut World, id: &str, previous: Entity, definition: WorkspaceDefinition) -> Result<(), WorkspaceError>
    开始重载：私有函数，验证请求、旧Workspace和新定义身份一致，关闭旧实例后开始已验证启动

begin_workspace_start(
    world: &mut World,
    id: String,
    definition: WorkspaceDefinition,
    kind: PendingWorkspaceKind,
)
    开始启动：私有函数，复核定义、创建Workspace Entity并请求全部AgentImage
    行为：
        验证id、Workspace名称、绝对project_root、非空agents和唯一Agent名称
        验证manager对应一个Agent定义
        Agent名称必须能够安全用于默认Memory目录段
        memory_path存在时必须是Compose解析后的绝对路径
        相同WorkspaceKey已经登记或正在启动时返回DuplicateWorkspace
        创建暂存Workspace Entity并插入WorkspaceIdentity和WorkspaceConfiguration
        调用WorldInferenceExt::load_workspace_model_routes(workspace, project_root)
        项目级模型路由加载失败时despawn Workspace Entity并直接发布当前操作失败结果
        为每个Agent生成独立子请求ID并发送LoadAgentImage
        保存PendingWorkspace和所有image_requests路由
        不读取AgentImage组件，不提前创建部分AgentInstance

begin_workspace_start_validated(world: &mut World, id: &str, definition: WorkspaceDefinition, kind: PendingWorkspaceKind) -> Result<(), WorkspaceError>
    开始已验证启动：私有函数，拒绝重复Workspace，创建暂存Entity、加载项目模型路由并发送镜像请求

collect_agent_image_system(world: &mut World)
    收集镜像：私有System，读取LoadAgentImageResult并推进对应PendingWorkspace
    行为：
        根据结果id从image_requests定位Workspace请求与Agent名称
        成功时保存AgentImage Entity
        失败时保存转换后的AgentImageLoadFailed
        全部Agent都有结果且任一失败时调用fail_pending_workspace
        全部成功时调用prepare_workspace_agents
        prepare_workspace_agents失败时调用fail_pending_workspace

prepare_workspace_agents(world: &mut World, request_id: &str) -> Result<(), WorkspaceError>
    准备实例：私有函数，在发送任何CreateAgent前完整构造所有Agent材料
    行为：
        按WorkspaceAgentDefinition配置顺序处理每个Agent
        读取AgentImageIdentity、AgentImageSoul、AgentImageModelConfig和AgentImageDefaultVisibility
        调用WorldInferenceExt::build_agent_inference_snapshot构造AgentInferenceSnapshot
        构造default_visibility：镜像默认resources + definition.resources - definition.disable_resources
        根据agent_images_root和AgentImageReference构造镜像版本根
        使用definition.project_root和镜像版本根构造AgentToolEnvironment
        memory_path为空时生成<project>/.margatroid/workspaces/<workspace>/memory/<agent>/memory.sql
        调用AgentMemory::open取得AgentMemory与恢复的RealtimeContext
        收集Soul、default_visibility和恢复上下文
        构造PreparedWorkspaceAgent并保存到pending.prepared
        任一步失败时释放已打开AgentMemory并返回错误，不发送部分Agent创建事件
        全部Agent准备成功后为每个Agent生成稳定Agent ID和独立创建子请求ID
        保存agent_requests[子请求ID] = (Workspace请求ID, Agent名称)
        发送AgentCreateRequest { id, agent_id, workspace_id, system_prompt, messages, tool_context, default_visibility }
        AgentCreateRequest只交付AgentPlugin自有字段，不携带Memory、Inference或Tool组件

collect_agent_created_system(world: &mut World)
    收集创建回执：私有System，消费AgentCreated并补齐其他Plugin拥有的组件
    行为：
        根据AgentCreated.id从agent_requests定位Workspace请求与Agent名称
        从pending.prepared取出对应PreparedWorkspaceAgent
        验证AgentCreated.agent存活且AgentWorkspaceId指向本次暂存Workspace
        调用WorldMemoryExt::bind_agent_memory绑定AgentMemory和恢复的messages、tool_context
        插入PreparedWorkspaceAgent.inference_snapshot
        插入PreparedWorkspaceAgent.tool_environment
        调用validate_agent_visibility解析全部AgentDynamicVisibility资源并检查暴露名称唯一
        成功时保存pending.agents[Agent名称] = Agent Entity
        任一步失败时despawn当前Agent并调用fail_pending_workspace
        全部Agent完成时使用pending.agents构造WorkspaceAgents并确定manager
        挂载WorkspaceAgents并写入WorkspaceRegistry.ready；WorkspaceAgents存在即表示Workspace可接收请求
        清理pending、image_requests和agent_requests
        发布StartWorkspaceResult或ReloadWorkspaceResult成功结果

attach_prepared_agent(world: &mut World, request_id: &str, name: &str, agent: Entity) -> Result<(), WorkspaceError>
    绑定实例材料：私有函数，验证Agent归属，绑定Memory、Inference和Tool组件，并验证动态可见资源

validate_agent_visibility(world: &World, agent: Entity) -> Result<(), WorkspaceError>
    验证Agent可见性：私有函数，确保启动成功时全部动态可见资源可解析为名称唯一的工具
    行为：
        读取AgentDynamicVisibility；缺失时返回ResourceSetupFailed
        按ResourceRef顺序逐个调用WorldToolExt::resolve_tool
        Provider未注册、Skill或Workflow文件不存在、定义非法时返回ResourceSetupFailed
        两个资源暴露相同ToolDefinition.name时返回ResourceSetupFailed
        只验证并丢弃Tool，不缓存工具定义或资源正文

complete_pending_workspace(world: &mut World, request_id: &str) -> Result<(), WorkspaceError>
    完成启动：私有函数，挂载WorkspaceAgents、登记ready索引、清理子请求并发布成功结果

fail_pending_workspace(world: &mut World, request_id: &str, error: WorkspaceError)
    终止启动：私有函数，释放本次未完成Workspace拥有的运行时对象
    行为：
        丢弃PreparedWorkspaceAgent中尚未绑定的AgentMemory
        despawn pending.agents中已经创建并完成部分绑定的全部Agent
        despawn本次暂存Workspace Entity
        清理image_requests、agent_requests和pending
        Start发送StartWorkspaceResult::Err
        Reload发送ReloadWorkspaceResult::Err；旧Workspace已经关闭，不做回滚
        AgentImage Entity由AgentImageLoaderPlugin拥有，不释放

stop_workspace_inner(world: &mut World, workspace: Entity) -> Result<(), WorkspaceError>
    关闭Workspace：私有函数，停止查询并释放Workspace拥有的AgentInstance
    行为：
        验证workspace存活、包含WorkspaceIdentity和WorkspaceAgents且已登记在WorkspaceRegistry.ready
        从WorkspaceRegistry.ready移除对应WorkspaceKey
        依次despawn全部Agent Entity，使AgentMemory连接随组件释放
        despawn Workspace Entity
        不删除memory.sql、项目文件或AgentImage Entity

validate_request_id(world: &World, id: &str) -> Result<(), WorkspaceError>
    验证请求ID：私有函数，拒绝空ID和已处于pending的ID

validate_definition(definition: WorkspaceDefinition) -> Result<WorkspaceDefinition, WorkspaceError>
    验证定义：私有函数，规范化项目根和Memory路径，验证Agent唯一性与manager引用

validate_agent_definition(agent: &WorkspaceAgentDefinition) -> Result<(), WorkspaceError>
    验证Agent定义：私有函数，验证Agent逻辑名称

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

image_root(root: &Path, reference: &AgentImageReference) -> PathBuf
    构造镜像版本根：私有函数，依次拼接scope、name和tag

default_memory_path(project_root: &Path, workspace: &str, agent: &str) -> PathBuf
    构造默认Memory路径：私有函数，返回项目内对应Agent的memory.sql路径

agent_image_components_missing() -> WorkspaceError
    构造镜像组件错误：私有函数，返回稳定AgentImageComponentsMissing错误

ready_workspace_key(world: &World, workspace: Entity) -> Result<WorkspaceKey, WorkspaceError>
    取得就绪键：私有函数，验证Entity、Identity、Agents和ready索引一致

is_registered_workspace(world: &World, workspace: Entity) -> bool
    检查已登记Workspace：私有函数，复用ready_workspace_key

pending_contains_key(world: &World, key: &WorkspaceKey) -> bool
    检查待处理Workspace：私有函数，查询是否存在相同WorkspaceKey的pending操作

cleanup_orphan_agent(world: &mut World, agent: Entity)
    清理孤立Agent：私有函数，仅在Agent的Workspace已经失活时释放Agent
```

## 逻辑

```text
安装：
    AgentImageLoaderPlugin
        -> InferencePlugin
        -> ToolPlugin
        -> MemoryPlugin
        -> AgentPlugin
        -> WorkspacePlugin::open(agent_images_root)
    WorkspacePlugin只协调已有Plugin，不替它们重新实现加载、验证或执行
    WorkspacePlugin不解析YAML，不读取Skill/Workflow正文，不执行推理、工具或Agent消息循环

输入与收集边界：
    Compose
        -> 构造WorkspaceDefinition和WorkspaceAgentDefinition
        -> 不创建Entity，不加载AgentImage，不打开Memory
    AgentImageLoaderPlugin
        -> AgentImage Entity、Soul、中立模型配置和镜像默认资源名称
    InferencePlugin
        -> Workspace项目级模型路由和AgentInferenceSnapshot
    ToolPlugin
        -> AgentToolEnvironment
        -> 工具定义在每次InferenceCommand发送前由AgentPlugin按动态可见性解析
    MemoryPlugin
        -> AgentMemory
        -> 打开数据库时恢复的RealtimeContext { messages, tool_context }
    WorkspacePlugin自身
        -> Workspace归属、逻辑名称和Agent Entity索引
        -> 合并后把default_visibility交给AgentCreateRequest，由AgentPlugin构造两个可见性组件
    Skill和Workflow在启动完成前解析一次以验证动态可见性
        -> Workspace传递项目根与镜像根并调用ToolPlugin真实解析路径
        -> 验证结果和正文不缓存
        -> 每次构造InferenceCommand时仍由对应ToolDefinitionProvider读取当前内容

workspace up：
    Compose生成WorkspaceDefinition
        -> world.start_workspace(id, definition)
        -> begin_workspace_command_system校验并创建暂存Workspace Entity
        -> WorldInferenceExt::load_workspace_model_routes(workspace, project_root)
        -> 发送LoadAgentImage * N
        -> collect_agent_image_system收集LoadAgentImageResult * N
        -> 全部镜像成功后读取Soul、模型配置和默认可见性
        -> 合并Workspace资源配置，构造Inference、Tool、Memory实例材料
        -> 全部材料成功后发送AgentCreateRequest * N
        -> AgentPlugin创建Agent Entity并发送AgentCreated * N
        -> Workspace绑定AgentMemory、AgentInferenceSnapshot和AgentToolEnvironment
        -> 逐Agent验证全部动态可见资源存在、定义有效且暴露名称唯一
        -> 全部Agent完成后挂载WorkspaceAgents并写入WorkspaceRegistry.ready
        -> 发布StartWorkspaceResult::Ok

项目级模型路由：
    WorkspacePlugin传入已经规范化的project_root
        -> InferencePlugin读取<project_root>/.margatroid/models.toml
        -> 存在时编译并挂载WorkspaceModelRoutes
        -> 不存在时移除旧WorkspaceModelRoutes并返回0
    每次InferenceCommand：
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
        -> 按workspace up流程创建新Workspace和Agent
        -> 新启动失败时发布ReloadWorkspaceResult::Err，旧Workspace不恢复
        -> 当前阶段不同时运行新旧实例，不实现无停机热切换

workspace down：
    world.stop_workspace(id, workspace)
        -> 验证Workspace已登记且包含WorkspaceAgents
        -> 从WorkspaceRegistry.ready移除索引
        -> despawn全部AgentInstance
        -> despawn Workspace Entity
        -> 保留memory.sql、项目文件和AgentImage Entity

所有权与边界：
    WorkspacePlugin拥有Workspace Entity和由它启动的AgentInstance
    AgentImageLoaderPlugin拥有AgentImage Entity
    MemoryPlugin拥有SQLite格式与AgentMemory组件行为
    InferencePlugin拥有WorkspaceModelRoutes和AgentInferenceSnapshot
    ToolPlugin拥有AgentToolEnvironment类型，WorkspacePlugin只负责构造实例值
    AgentPlugin拥有AgentWorkspaceId、AgentContext、AgentStatus和两层可见性
    各业务Plugin拥有自己挂在Workspace或Agent上的typed Component语义
    WorkspacePlugin只拥有编排决策，不复制其他Plugin的运行时状态
```

## 持有关系

```text
App
└── World
    ├── WorkspaceRegistry Resource
    │   ├── agent_images_root
    │   ├── ready: HashMap<WorkspaceKey, Entity>
    │   └── pending: HashMap<String, PendingWorkspace>
    ├── Workspace Entity
    │   ├── WorkspaceIdentity
    │   ├── WorkspaceConfiguration
    │   ├── WorkspaceAgents
    │   ├── WorkspaceModelRoutes--InferencePlugin拥有
    │   └── AgentInstance Entity * N
    │       ├── AgentWorkspaceId--AgentPlugin拥有
    │       ├── AgentContext / AgentStatus--AgentPlugin拥有
    │       ├── AgentInferenceSnapshot--InferencePlugin拥有
    │       ├── AgentToolEnvironment--ToolPlugin拥有类型
    │       ├── AgentDefaultVisibility / AgentDynamicVisibility--AgentPlugin拥有
    │       └── AgentMemory--MemoryPlugin拥有
    └── AgentImage Entity * N
        ├── AgentImageIdentity--AgentImageLoaderPlugin拥有
        ├── AgentImageSoul--AgentImageLoaderPlugin拥有
        ├── AgentImageModelConfig--AgentImageLoaderPlugin拥有
        └── AgentImageDefaultVisibility--AgentImageLoaderPlugin拥有

启动事件链：
StartWorkspace
    -> LoadAgentImage * N
       -> LoadAgentImageResult * N
          -> PreparedWorkspaceAgent * N
             -> AgentCreateRequest * N
                -> AgentCreated * N
                   -> Workspace绑定Memory、InferenceSnapshot和ToolEnvironment
                   -> 全部完成后Workspace可接收请求
```
