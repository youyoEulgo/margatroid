# WorkspacePlugin

## 定位

WorkspacePlugin是Workspace运行时的编排者。它接收Compose已经编译完成的`WorkspaceDefinition`，
从各业务Plugin收集创建AgentInstance所需的信息，最后发送Agent创建事件。

WorkspacePlugin负责两类事情：

- 启动、重载和关闭Workspace中的整组AgentInstance。
- 在Workspace和Agent Entity上保存稳定、只读、可查询的运行时关系。

WorkspacePlugin不解析YAML，不读取Skill/Workflow正文，不执行推理、工具或Agent消息循环，也不把
其他Plugin的运行状态复制到一个通用字典。信息仍由拥有它的Plugin用typed Component保存；
WorkspacePlugin只建立Workspace归属、逻辑名称与Entity之间的关系。

## 输入与收集范围

`WorkspaceDefinition`和`WorkspaceAgentDefinition`定义在`margatroid_types`，由Compose构造：

```text
WorkspaceDefinition
├── name
├── project_root
├── manager
└── agents
    ├── name
    ├── image
    ├── resources / disable_resources
    └── memory_path
```

WorkspacePlugin为每个Agent收集：

```text
AgentImageLoaderPlugin
    AgentImage Entity、Soul、中立模型配置、默认ResourceRef可见性

InferencePlugin
    Workspace项目级模型路由、AgentInferenceSnapshot

ToolPlugin
    AgentToolEnvironment；工具定义在每次LLM请求前由动态可见性解析

MemoryPlugin
    AgentMemory、realtime_messages恢复出的Vec<Message>

WorkspacePlugin自身
    Workspace归属；Agent默认可见性由AgentPlugin持有
```

Skill和Workflow正文不在启动阶段加载。Workspace只计算最终可见名称并把项目根、镜像根交给
ToolPlugin的执行环境；实际使用时由SkillPlugin或WorkflowPlugin动态读取当前内容。

## 类型

公开：
```text
WorkspacePlugin：Workspace运行编排插件，公开结构体--配置AgentImage根与处理Schedule
    agent_images_root: PathBuf--与AgentImageLoaderPlugin相同的镜像库根，私有
    schedule: String--命令与依赖结果收集所属Schedule，私有
    open(agent_images_root: impl Into<PathBuf>) -> Result<Self, WorkspaceError>
        打开插件：公开关联函数，规范化AgentImage根但不扫描或读取镜像
        限制：daemon组合根必须向WorkspacePlugin和AgentImageLoaderPlugin传入同一个镜像库根
    with_schedule(mut self, schedule: impl Into<String>) -> Self
        指定阶段：公开方法，替换默认RuntimePlugin::UPDATE并返回自身
    impl Plugin for WorkspacePlugin
        Plugin：公开trait实现
        build(self, app: &mut App)
            构建插件：安装WorkspaceRegistry并挂载Workspace生命周期System
            行为：
                确认RuntimePlugin、AgentImageLoaderPlugin、InferencePlugin、ToolPlugin、AgentPlugin与MemoryPlugin已安装
                确认schedule存在且WorkspacePlugin尚未安装
                插入WorkspaceRegistry
                挂载begin_workspace_command_system
                挂载collect_agent_image_system
                挂载collect_agent_created_system

StartWorkspace：启动Workspace事件，公开事件--从编译后的静态定义创建一组AgentInstance
    id: String--调用方生成的请求ID
    definition: WorkspaceDefinition--Compose产生的完整静态定义
    impl Event for StartWorkspace
        Event：公开trait实现

StartWorkspaceResult：启动Workspace结果，公开事件--返回Ready Workspace Entity或稳定错误
    id: String--原请求ID
    result: Result<Entity, WorkspaceError>--成功时为可以接收请求的Workspace Entity
    impl Event for StartWorkspaceResult
        Event：公开trait实现

ReloadWorkspace：重载Workspace事件，公开事件--关闭当前实例组并按新定义重新创建
    id: String--调用方生成的请求ID
    workspace: Entity--当前Ready Workspace Entity
    definition: WorkspaceDefinition--Compose重新编译的最新定义
    impl Event for ReloadWorkspace
        Event：公开trait实现

ReloadWorkspaceResult：重载Workspace结果，公开事件--成功时返回新的Workspace Entity
    id: String--原请求ID
    previous: Entity--被关闭的原Workspace Entity
    result: Result<Entity, WorkspaceError>--成功时为重建后的Ready Workspace Entity
    impl Event for ReloadWorkspaceResult
        Event：公开trait实现

StopWorkspace：关闭Workspace事件，公开事件--释放Workspace拥有的全部AgentInstance
    id: String--调用方生成的请求ID
    workspace: Entity--需要关闭的Ready Workspace Entity
    impl Event for StopWorkspace
        Event：公开trait实现

StopWorkspaceResult：关闭Workspace结果，公开事件
    id: String--原请求ID
    workspace: Entity--原Workspace Entity
    result: Result<(), WorkspaceError>--成功表示Workspace和其AgentInstance已经释放
    impl Event for StopWorkspaceResult
        Event：公开trait实现

WorkspaceIdentity：Workspace身份，公开组件--保存稳定逻辑名称与项目根
    name: Arc<str>--项目内Workspace逻辑名称，私有
    project_root: Arc<PathBuf>--规范化绝对项目根，私有
    name(&self) -> &str
        取得名称：公开方法
    project_root(&self) -> &Path
        取得项目根：公开方法
    impl Component for WorkspaceIdentity
        Component：公开trait实现

WorkspaceConfiguration：Workspace配置快照，公开组件--保存本次启动使用的编译后定义
    definition: Arc<WorkspaceDefinition>--不包含运行时Entity的静态定义，私有
    definition(&self) -> &WorkspaceDefinition
        取得定义：公开方法，返回只读引用
    impl Component for WorkspaceConfiguration
        Component：公开trait实现

WorkspaceStatus：Workspace公开状态，公开枚举
    Starting--正在收集镜像和实例材料，不可接收Agent请求
    Ready--全部Agent已经创建、补齐组件并绑定Memory
    Stopping--正在释放实例，不再接受新请求
    impl Clone + Copy + PartialEq + Eq for WorkspaceStatus
        值语义：公开trait实现

WorkspaceLifecycle：Workspace状态组件，公开组件
    status: WorkspaceStatus--当前状态，私有
    status(&self) -> WorkspaceStatus
        取得状态：公开方法
    impl Component for WorkspaceLifecycle
        Component：公开trait实现

WorkspaceAgents：Workspace Agent索引，公开组件--只在全部Agent准备成功后挂载
    manager: Entity--默认入口AgentInstance，私有
    agents: BTreeMap<String, Entity>--Agent逻辑名称到Entity的稳定映射，私有
    manager(&self) -> Entity
        取得默认入口：公开方法
    agent(&self, name: &str) -> Option<Entity>
        取得Agent：公开方法，按逻辑名称查询
    iter(&self) -> impl Iterator<Item = (&str, Entity)> + '_
        遍历Agent：公开方法，按逻辑名称顺序返回
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

WorkspaceError：Workspace启动和生命周期错误，公开结构体--提供不暴露绝对路径或资源正文的稳定描述
    kind: WorkspaceErrorKind--错误分类，私有
    message: String--不包含Soul、Memory消息、Skill正文或工具参数的有界描述，私有
    kind(&self) -> WorkspaceErrorKind
        取得分类：公开方法
    message(&self) -> &str
        取得描述：公开方法
    impl Clone for WorkspaceError
        Clone：公开trait实现
    impl fmt::Display for WorkspaceError
        Display：公开trait实现
    impl std::error::Error for WorkspaceError
        Error：公开trait实现

WorldWorkspaceExt：World Workspace扩展，公开trait--发送生命周期命令并查询运行时关系
    start_workspace(&self, id: impl Into<String>, definition: WorkspaceDefinition)
        启动Workspace：公开方法，发送StartWorkspace并唤醒Runtime
    reload_workspace(&self, id: impl Into<String>, workspace: Entity, definition: WorkspaceDefinition)
        重载Workspace：公开方法，发送ReloadWorkspace并唤醒Runtime
    stop_workspace(&self, id: impl Into<String>, workspace: Entity)
        关闭Workspace：公开方法，发送StopWorkspace并唤醒Runtime
    workspace(&self, project_root: &Path, name: &str) -> Option<Entity>
        查询Workspace：公开方法，只返回Ready且仍存活的Workspace Entity
    workspace_agent(&self, workspace: Entity, name: &str) -> Option<Entity>
        查询Agent：公开方法，从WorkspaceAgents按逻辑名称返回存活Agent
    workspace_manager(&self, workspace: Entity) -> Option<Entity>
        查询默认入口：公开方法，返回存活manager Agent
    workspace_of(&self, agent: Entity) -> Option<Entity>
        反查Workspace：公开方法，从AgentWorkspaceId返回存活Workspace
    impl WorldWorkspaceExt for World
        WorldWorkspaceExt for World：公开trait实现
```

crate公开：
```text
WorkspaceRegistry：Workspace注册与启动状态，crate公开Resource--保存Ready索引和未完成操作
    agent_images_root: Arc<PathBuf>--AgentImage版本目录推导根
    ready: HashMap<WorkspaceKey, Entity>--规范化项目根与Workspace名称到Ready Entity
    pending: HashMap<String, PendingWorkspace>--Workspace请求ID到未完成启动
    image_requests: HashMap<String, (String, String)>--镜像子请求ID到Workspace请求ID和Agent名称
    agent_requests: HashMap<String, (String, String)>--Agent创建子请求ID到Workspace请求ID和Agent名称
    impl Resource for WorkspaceRegistry
        Resource：crate公开trait实现
```

私有：
```text
WorkspaceKey：Workspace注册键，私有结构体--由规范化project_root和name组成
    project_root: PathBuf
    name: String

PendingWorkspaceKind：未完成操作类型，私有枚举
    Start
    Reload { previous: Entity }

PendingWorkspace：未完成Workspace启动，私有结构体
    kind: PendingWorkspaceKind--决定最终发布Start或Reload结果
    workspace: Entity--本次新建的Starting Workspace Entity
    definition: WorkspaceDefinition--已复核定义
    images: BTreeMap<String, Result<Entity, WorkspaceError>>--每个Agent的镜像收集结果
    prepared: BTreeMap<String, PreparedWorkspaceAgent>--全部镜像成功后构造的实例材料
    agents: BTreeMap<String, Entity>--已经收到回执并完成其他Plugin组件绑定的Agent

PreparedWorkspaceAgent：已经收集完成的单Agent实例材料，私有结构体
    name: String--Agent逻辑名称
    image: Entity--来源AgentImage Entity
    image_reference: AgentImageReference--来源镜像引用
    system_prompt: String--从AgentImage Soul取得的系统提示词
    messages: Vec<Message>--MemoryPlugin恢复的实时上下文
    default_visibility: AgentDefaultVisibility--镜像默认值与Workspace参数的合并结果
    inference_snapshot: AgentInferenceSnapshot--InferencePlugin构造的实例推理快照
    tool_environment: AgentToolEnvironment--项目根与镜像版本根
    memory: AgentMemory--已经打开但尚未绑定Entity的SQLite连接
```

## 函数

私有：
```text
begin_workspace_command_system(world: &mut World)
    开始命令：私有System，读取StartWorkspace、ReloadWorkspace和StopWorkspace
    行为：
        StartWorkspace调用begin_workspace_start
        ReloadWorkspace先验证旧Workspace为Ready、新definition合法且WorkspaceKey与旧身份一致
        验证成功后调用stop_workspace_inner关闭旧实例，再以Reload类型调用begin_workspace_start
        StopWorkspace调用stop_workspace_inner并立即发送StopWorkspaceResult

begin_workspace_start(
    world: &mut World,
    id: String,
    definition: WorkspaceDefinition,
    kind: PendingWorkspaceKind,
)
    开始启动：私有函数，复核定义、创建Workspace Entity并请求全部AgentImage
    行为：
        验证id、Workspace名称、绝对project_root、非空agents和唯一Agent名称
        id不能与其他pending Workspace操作重复
        验证manager对应一个Agent定义
        Agent名称必须能够安全用于默认Memory目录段
        memory_path存在时必须是Compose解析后的绝对路径
        相同WorkspaceKey已经Ready或正在启动时返回DuplicateWorkspace
        创建Workspace Entity并插入WorkspaceIdentity、WorkspaceConfiguration与Starting状态
        调用WorldInferenceExt::load_workspace_model_routes(workspace, project_root)
        项目级模型路由加载失败时despawn Workspace Entity并直接发布当前操作失败结果
        为每个Agent生成独立子请求ID并发送LoadAgentImage
        保存PendingWorkspace和所有image_requests路由
        不读取AgentImage组件，不提前创建部分AgentInstance

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
    行为：对每个WorkspaceAgentDefinition按配置顺序执行
        读取AgentImageIdentity、AgentImageSoul、AgentImageModelConfig和AgentImageDefaultVisibility
        调用WorldInferenceExt::build_agent_inference_snapshot构造AgentInferenceSnapshot
        构造AgentDefaultVisibility.resources：镜像默认resources + definition.resources - definition.disable_resources
        根据agent_images_root和AgentImageReference构造镜像版本根
        使用definition.project_root和镜像版本根构造AgentToolEnvironment
        AgentPlugin创建Entity时将AgentDefaultVisibility复制为AgentDynamicVisibility
        memory_path为空时生成<project>/.margatroid/workspaces/<workspace>/memory/<agent>/memory.sql
        调用AgentMemory::open取得AgentMemory与恢复messages
        收集Soul、AgentDefaultVisibility和恢复上下文
        构造PreparedWorkspaceAgent并保存到pending.prepared
        任一步失败时释放已打开AgentMemory并返回错误，不发送部分Agent创建事件
    全部Agent准备成功后：
        为每个PreparedWorkspaceAgent生成独立创建子请求ID
        保存agent_requests[子请求ID] = (Workspace请求ID, Agent名称)
        发送AgentCreateRequest { id, workspace_id, system_prompt, messages, default_visibility }
        AgentCreateRequest只交付AgentPlugin自有字段，不携带Memory、Inference或Tool组件

collect_agent_created_system(world: &mut World)
    收集Agent创建回执：消费AgentCreated并补齐其他Plugin拥有的组件
    行为：
        根据AgentCreated.id从agent_requests定位Workspace请求与Agent名称
        从pending.prepared取出对应PreparedWorkspaceAgent
        验证AgentCreated.agent存活且AgentWorkspaceId指向本次Starting Workspace
        调用WorldMemoryExt::bind_agent_memory绑定AgentMemory和恢复messages
        插入PreparedWorkspaceAgent.inference_snapshot
        插入PreparedWorkspaceAgent.tool_environment
        成功时保存pending.agents[Agent名称] = Agent Entity
        任一步失败时despawn当前Agent并调用fail_pending_workspace
        全部Agent完成时：
            使用pending.agents构造WorkspaceAgents并确定manager Entity
            把WorkspaceLifecycle切换为Ready
            写入WorkspaceRegistry.ready
            清理pending、image_requests和agent_requests
            发布StartWorkspaceResult或ReloadWorkspaceResult成功结果

fail_pending_workspace(world: &mut World, request_id: &str, error: WorkspaceError)
    终止启动：私有函数，释放本次未完成Workspace拥有的运行时对象
    行为：
        丢弃PreparedWorkspaceAgent中尚未绑定的AgentMemory
        despawn pending.agents中已经创建并完成部分绑定的全部Agent
        despawn本次Starting Workspace Entity
        清理image_requests、agent_requests和pending
        Start发送StartWorkspaceResult::Err
        Reload发送ReloadWorkspaceResult::Err；旧Workspace已经关闭，不做回滚
        AgentImage Entity由AgentImageLoaderPlugin拥有，不释放

stop_workspace_inner(world: &mut World, workspace: Entity) -> Result<(), WorkspaceError>
    关闭Workspace：私有函数，停止查询并释放Workspace拥有的AgentInstance
    行为：
        验证workspace存活、状态为Ready且包含WorkspaceIdentity与WorkspaceAgents
        从WorkspaceRegistry.ready移除对应WorkspaceKey
        将状态替换为Stopping
        依次despawn全部Agent Entity，使AgentMemory连接随组件释放
        despawn Workspace Entity
        不删除memory.sql、项目文件或AgentImage Entity

normalize_project_root(path: PathBuf) -> Result<PathBuf, WorkspaceError>
    规范化项目根：私有函数，要求绝对路径且不包含父级跳转

validate_logical_name(value: &str) -> Result<(), WorkspaceError>
    验证Workspace或Agent逻辑名称：私有函数，要求非空、长度有界且可安全作为单个目录段
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

workspace up：
    Compose读取margatroid-workspace.yaml
        -> 生成WorkspaceDefinition
        -> world.start_workspace(id, definition)
    WorkspacePlugin创建Starting Workspace Entity
        -> 加载项目级模型路由
        -> 为全部Agent发送LoadAgentImage
    全部AgentImage成功
        -> 读取Soul、模型配置和默认可见性
        -> 合并AgentDefaultVisibility
        -> 构造Inference、统一默认可见性、ToolEnvironment与Memory实例材料
        -> 全部材料成功后才发送CreateAgent * N
    发送Agent创建事件后的Entity收集、其他Plugin组件挂载和Ready切换协议暂不定义

运行时查询：
    ServerPlugin取得Workspace Entity
        -> workspace_manager(workspace)找到默认Agent
    Agent或其他Plugin持有Agent Entity
        -> 读取AgentWorkspaceId反查Workspace
        -> 读取WorkspaceIdentity取得项目根
        -> 读取各业务Plugin挂载的typed Component取得实际配置
    WorkspacePlugin不提供String到Any的通用属性表

workspace reload：
    Compose重新读取当前Workspace文件并生成新WorkspaceDefinition
        -> world.reload_workspace(id, old_workspace, definition)
    WorkspacePlugin先停止旧Workspace及全部Agent
        -> 关闭旧AgentMemory连接
        -> 再按workspace up流程创建新Workspace Entity与Agent Entity
    新启动失败时返回错误，旧Workspace不恢复
    当前阶段不同时运行新旧实例，不实现无停机热切换

workspace down：
    world.stop_workspace(id, workspace)
        -> 从Ready查询索引移除
        -> despawn全部AgentInstance
        -> despawn Workspace Entity
        -> 保留Memory与全部资源文件

所有权：
    WorkspacePlugin拥有Workspace Entity和由它启动的AgentInstance生命周期
    AgentImageLoaderPlugin拥有AgentImage Entity
    MemoryPlugin拥有SQLite格式与AgentMemory组件行为
    各业务Plugin拥有自己挂在Workspace或Agent上的Component语义
    WorkspacePlugin只拥有Workspace编排决策，创建时把AgentDefaultVisibility交给AgentPlugin
    ToolPlugin拥有AgentToolEnvironment类型，WorkspacePlugin只负责构造实例值
```

## 持有关系

```text
App
└── World
    ├── WorkspaceRegistry Resource
    │   ├── ready: HashMap<WorkspaceKey, Entity>
    │   └── pending: HashMap<String, PendingWorkspace>
    └── Workspace Entity
        ├── WorkspaceIdentity
        ├── WorkspaceConfiguration
        ├── WorkspaceLifecycle::Ready
        ├── WorkspaceAgents
        ├── WorkspaceModelRoutes--InferencePlugin拥有
        └── AgentInstance Entity * N
            ├── AgentWorkspaceId--AgentPlugin拥有
            ├── AgentContext / AgentStatus--AgentPlugin拥有
            ├── AgentInferenceSnapshot--InferencePlugin拥有
            ├── AgentToolEnvironment--ToolPlugin拥有类型
            ├── AgentDefaultVisibility / AgentDynamicVisibility--AgentPlugin拥有
            └── AgentMemory--MemoryPlugin拥有

启动事件链：
StartWorkspace
    -> LoadAgentImage * N
       -> LoadAgentImageResult * N
          -> PreparedWorkspaceAgent * N
             -> Agent创建事件 * N
                -> AgentCreated * N
                   -> Workspace绑定AgentMemory、AgentInferenceSnapshot和AgentToolEnvironment
                   -> 全部完成后Workspace Ready
```
