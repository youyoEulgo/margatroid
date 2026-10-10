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

正文按 `lib`、`system`、`handler`、`events`、`types`、`error` 六个一级标题组织。

```text
lib        src/lib.rs        Workspace组件、Plugin和世界扩展
system     src/system.rs     事件读取与Handler调度
handler    src/handler.rs    启动、停止的事件处理
events     共享types crate    workspace_events.rs中的事件契约，本crate只使用或重导出
types      src/types.rs      Workspace状态机和操作进度
error      共享错误契约       错误定义归属见error板块
```

**本文是目标设计，尚未同步到代码。本次只修改Workspace设计文档。**

# lib

## 组件定义位置

Workspace图书馆组件及其实现方法；WorkspaceState等组件公开关联类型随组件契约组织，计划写入共享types crate的`workspace_components.rs`。对应公开事件写入各自的`*_events.rs`，不在插件实现crate重复定义。

此处只标注未来源码归属，尚未创建或迁移源码文件。文档仍在lib板块描述图书馆类型和资源类型；Plugin、System、Handler及插件私有加载状态不随组件迁入共享文件。

## 类型

公开：
```text
Workspace：工作区图书馆组件，公开Component--保存工作区自身身份，与镜像资源集合共同挂载到Workspace Entity
    resourceid: ResourceID--工作区自身地址，由LibraryComponent提供查询
    definition: Arc<WorkspaceDefinition>--静态工作区定义
    project_root: Arc<PathBuf>--项目根目录
    agents: BTreeMap<ResourceID, Entity>--已成功启动的成员Agent地址到Entity
    state: WorkspaceState--工作区状态机，包含当前启动或停止过程
    definition(&self) -> &WorkspaceDefinition
        读取定义：公开方法，返回静态定义
    project_root(&self) -> &Path
        读取项目根：公开方法，返回项目路径
    state(&self) -> &WorkspaceState
        读取工作区状态：公开方法，返回state
    iter(&self) -> impl Iterator<Item = (&ResourceID, Entity)> + '_
        遍历成员：公开方法，返回已登记的Agent地址和实体，不过滤失效实体
    impl Component for Workspace
    impl LibraryComponent for Workspace
        LibraryComponent：公开trait实现
        resource_id(&self) -> &ResourceID
            读取自身地址：返回resourceid字段

WorkspacePlugin：工作区生命周期插件，公开结构体
    schedule: String--运行计划名称
    with_schedule(mut self, schedule: impl Into<String>) -> Self
        设置计划：公开方法，保存计划名并返回自身
    impl Default for WorkspacePlugin
        Default：公开trait实现
        default() -> Self
            构造默认插件：使用RuntimePlugin::UPDATE
    impl Plugin for WorkspacePlugin
        Plugin：公开trait实现
        build(self, app: &mut App)
            安装插件：公开trait方法
            行为：
                要求运行计划和公共事件、统一ResourcePlugin可用
                通过register_library::<Workspace>注册workspace地址类型
                image地址类型由镜像领域注册为ResourceSet<AgentImage>，不在此重复注册
                挂载start_workspace_system和stop_workspace_system
                不插入WorkspaceRegistry
                不通过Agent、AgentImage、Memory、Tool、MCL或LuaRuntime的插件安装标记建立业务插件依赖
```

## 扩展 trait

公开：
```text
WorldWorkspaceExt：工作区世界扩展，公开trait
    start_workspace(&self, id: impl Into<String>, definition: WorkspaceDefinition)
        请求启动：公开方法，发送StartWorkspace
    stop_workspace(&self, id: impl Into<String>, workspace: Entity)
        请求停止：公开方法，发送StopWorkspace
    workspace_by_id(&self, id: &ResourceID) -> Result<Entity, WorkspaceError>
        查询工作区：公开方法，要求地址类型为workspace，调用ResourceID::find_entities并确认目标挂载Workspace
        行为：无匹配返回WorkspaceNotAlive，唯一匹配返回Entity，多匹配返回ResourceResolutionFailed，不任取第一个
    workspaces(&self) -> Vec<Entity>
        查询工作区：公开方法，查询挂载Workspace组件的存活Entity，不读取注册表
    impl WorldWorkspaceExt for World
        World：公开trait实现，按上述事件发送和查询规则实现
```

## 逻辑

```text
数据归属：
    Workspace保存自己的resourceid、定义、成员引用和工作区状态
    definition.id用于创建时初始化resourceid，挂载后自身身份以LibraryComponent::resource_id为准，不允许二者被独立修改
    agents使用ResourceID作键，不再维护按成员名称查询的索引或扩展方法
    成员名称可用于配置和展示，不作为运行时成员寻址键
    镜像条目保存在同一Entity上的ResourceSet<AgentImage>中，Workspace内部不再保存agent_images
    ResourceSet的键保存镜像ResourceID，条目是AgentImage内容，不是另一个镜像Entity
    Workspace和ResourceSet<AgentImage>分别实现LibraryComponent与ResourcesComponent，允许共同挂载
    state是整个工作区的一个状态，不按成员名称查询，也不需要Option返回值
    没有manager，没有成员运行状态表
    workspace_of由Agent侧提供，不属于本trait

职责边界：
    Workspace协调启动与停止，不打开Memory、不组装Lua VM、不执行工具或推理
    Workspace不转发AgentControl、轮次中止或MCL业务命令
    成员运行状态由Agent负责，Workspace只接收本次启动和停止操作的结果
    镜像路径解析、加载、更新由AgentImage插件负责，WorkspacePlugin不再接收agent_images_root或提供open(root)
```

# system

## 函数

crate公开：
```text
start_workspace_system(world: &mut World)
    启动System：crate公开System
    处理事件：StartWorkspace、LoadAgentImageResult、StartAgentResult
    产出事件：LoadAgentImage、StartAgent、StartWorkspaceResult
    行为：
        克隆本帧全部StartWorkspace，逐个调用handle_start_workspace
        克隆本帧全部LoadAgentImageResult，逐个调用handle_agent_image_result
        克隆本帧全部StartAgentResult，逐个调用handle_start_agent_result

stop_workspace_system(world: &mut World)
    停止System：crate公开System
    处理事件：StopWorkspace、StopAgentResult
    产出事件：StopAgent、StopWorkspaceResult
    行为：
        克隆本帧全部StopWorkspace，逐个调用handle_stop_workspace
        克隆本帧全部StopAgentResult，逐个调用handle_stop_agent_result
```

## 逻辑

```text
事件驱动：
    System只读取事件并调用Handler，业务处理和事件发送在Handler展开
    没有事件时不扫描工作区判断是否完成
    最后一个必要结果到达时，在该事件的Handler中完成状态转换和结果发送
    启动System接受三种事件、产出三种事件，停止System接受两种事件、产出两种事件
    warning是日志，不另增事件种类

移除的System：
    begin_workspace_command_system
    reload_workspace_system
    collect_agent_image_system
    collect_agent_initialization_system及其曾计划的新名agent_status_system
    collect_workspace_stop_system
    route_agent_turn_abort_system
    route_mcl_command_system
    collect_mcl_command_response_system
```

# handler

## 函数

crate公开：
```text
handle_start_workspace(world: &mut World, request: StartWorkspace)
    处理启动请求：crate公开函数
    行为：
        校验请求ID和工作区定义，失败发送StartWorkspaceResult的Err
        按definition.id调用find_entities检查已有工作区，已存在时发送DuplicateWorkspace错误，不覆盖旧实例
        创建Entity，通过受检图书馆挂载入口插入Workspace，resourceid取definition.id，agents为空，state为Starting
        不再挂载独立ResourceId组件；镜像集合由镜像加载方在目标Workspace Entity上创建或更新
        在Starting中保存原始启动请求ID，并按镜像地址归并需要该镜像的成员Agent地址
        为每个不同镜像生成唯一加载子请求ID，在状态中登记关联后发送LoadAgentImage，携带目标Workspace完整Entity和镜像ResourceID
        无成员时直接置Ready并发送StartWorkspaceResult的Ok(workspace)
        有成员时不提前发送启动成功

handle_agent_image_result(world: &mut World, event: LoadAgentImageResult)
    处理镜像结果：crate公开函数
    行为：
        通过结果的子请求ID查找Starting中等待该结果的Workspace，未匹配时忽略
        核对目标Workspace完整Entity及镜像ResourceID，拒绝不匹配结果，不消费合法请求的等待记录
        匹配后移除对应镜像等待记录，重复结果不再次处理
        失败时记录工作区启动错误，不启动依赖该镜像的成员
        成功时确认目标Workspace Entity上的ResourceSet<AgentImage>包含该镜像ID；缺失则记录ResourceResolutionFailed且不启动对应成员
        LoadAgentImageResult只是挂载完成通知，挂载由AgentImage插件完成，Workspace不重复插入或替换镜像集合
        不登记依赖者，不创建镜像实体，也不在Workspace组件内另存镜像映射
        为等待该镜像的每个成员生成唯一StartAgent子请求ID，登记成员关联后发送StartAgent
        StartAgent携带工作区Entity、成员定义和镜像ResourceID，以工作区Entity加镜像ID定位具体镜像条目
        不由Workspace复制镜像内部的VM或模型数据，不以全局镜像查询的首个Entity代替指定工作区
        调用finish_workspace_start检查本次启动是否已完成

handle_start_agent_result(world: &mut World, event: StartAgentResult)
    处理成员启动结果：crate公开函数
    行为：
        通过结果的子请求ID查找Starting中等待该结果的Workspace，未匹配时忽略
        按等待记录确认成员身份，消费匹配记录，重复结果不再次处理
        成功时将成员Agent地址和Entity写入agents
        失败时记录工作区启动错误
        调用finish_workspace_start检查本次启动是否已完成

handle_stop_workspace(world: &mut World, request: StopWorkspace)
    处理停止请求：crate公开函数
    行为：
        检查目标Entity及Workspace组件，不存在时记录warning并发送WorkspaceNotAlive错误结果，不留下无回复请求
        Starting时返回WorkspaceBusy，不打断尚未收齐结果的启动过程
        Stopping时返回WorkspaceBusy，不重复发送成员停止请求
        Ready或Failed时，取出agents中的所有成员，保存原停止请求ID并将state置为Stopping
        为每个成员生成唯一StopAgent子请求ID，在Stopping中登记关联后发送StopAgent
        agents为空时直接回收Workspace并发送StopWorkspaceResult的Ok(原Entity)
        不读取Agent内部的VM、工具调用、推理或生命周期字段

handle_stop_agent_result(world: &mut World, event: StopAgentResult)
    处理成员停止结果：crate公开函数
    行为：
        通过结果的子请求ID查找Stopping中等待该结果的Workspace，未匹配时忽略
        按等待记录确认目标成员，消费匹配记录，重复结果不再次处理
        成功时从agents移除对应成员，Agent实体由Agent插件清理，不在此重复despawn
        失败时记录停止错误并保留该成员引用
        调用finish_workspace_stop检查本次停止是否已完成
```

私有：
```text
finish_workspace_start(world: &mut World, workspace: Entity)
    完成启动判定：私有函数，只由启动事件Handler调用
    行为：
        Starting中还有等待镜像或等待成员请求时返回，不扫描Agent状态
        全部请求结束且没有错误时将state置Ready，发送原始id对应的StartWorkspaceResult::Ok(workspace)
        全部请求结束且有错误时将state置Failed，发送原始id对应的StartWorkspaceResult::Err(error)
        失败时保留已创建成员和已加载的镜像集合，允许随后通过StopWorkspace清理，不偷偷销毁仍有成员的Workspace
        转移状态后不再保留本次启动等待记录，原请求只回复一次

finish_workspace_stop(world: &mut World, workspace: Entity)
    完成停止判定：私有函数，只由停止事件Handler调用
    行为：
        Stopping中还有等待成员请求时返回
        全部请求结束且没有错误时保存原始请求id与完整Workspace Entity，despawn Workspace，再发送StopWorkspaceResult::Ok(原Entity)
        全部请求结束且有错误时将state置Failed，保留未成功停止的成员，发送StopWorkspaceResult::Err(error)
        Workspace Entity销毁时，其Workspace组件与ResourceSet<AgentImage>一同释放
        不调用release_resource_owner，不登记ResourceOwners，不等待GC，也不删除磁盘镜像包
```

## 逻辑

```text
启动：
    StartWorkspace -> Starting -> LoadAgentImage
    LoadAgentImageResult -> 确认本Entity镜像集合已更新 -> StartAgent
    StartAgentResult -> 更新agents -> 全部成功后Ready -> StartWorkspaceResult
    同一工作区的多个成员使用同一镜像时只提交一次加载，加载完成后分别启动成员
    不同工作区可以各自保存同ID镜像条目，一方加载更新不会自动覆盖另一方集合
    StartWorkspaceResult成功意味着定义中的全部成员都取得StartAgentResult成功结果，不再是仅创建了Workspace Entity
    StartAgentResult成功的准确边界由Agent插件保证，Workspace不额外监听Agent初始化事件

停止：
    StopWorkspace -> Stopping -> StopAgent
    StopAgentResult -> 更新成员集合 -> 全部成功后销毁Workspace -> StopWorkspaceResult
    多成员工作区必须等待所有已发StopAgent请求结束，不能收到第一个结果就销毁Workspace
    StopAgentResult成功必须表示Agent已完成清理，不能仅表示接受了停止请求
    工具、推理、VM如何停止由Agent插件负责，Workspace不以轮询代替完成事件
    成功结果中的Entity表示已清理的实例身份，不再保证能查询其组件

过程关联：
    顶层请求id与子请求id分开，子请求id在运行期唯一，不只用成员名或可复用的实体index构造
    不使用全局WorkspaceRegistry，以Workspace.state中的等待表匹配结果
    可在结果事件到达时查询Workspace组件寻找匹配的子请求，不增加每帧轮询System
    一条等待记录只消费一次，旧结果不能推进新实例的操作

尚待确认的异常策略：
    本文为闭合流程暂定Starting和Stopping中的新停止请求返回WorkspaceBusy，不引入启动中取消协议
    启动失败收齐已发请求后返回Err并保留部分实例；停止失败保留未清理成员，后续可再次Stop
    不存在的Workspace按要求记录warning，同时暂定返回Err以完成请求关联
    这些是成功链路之外的补充约定；如需启动中停止、失败自动回滚或停止幂等成功，后续再设计
    未收到必要结果时保持对应状态，本次不引入超时或强制销毁策略
```

# events

## 类型

公开，统一定义于共享types crate的workspace_events.rs：
```text
StartWorkspace：Workspace启动请求，公开事件
    id: String--请求ID
    definition: WorkspaceDefinition--Workspace的定义
    impl Event for StartWorkspace

StartWorkspaceResult：Workspace启动结果，公开事件
    id: String--原请求ID
    result: Result<Entity, WorkspaceError>--成功时返回全部成员启动成功的Workspace Entity
    impl Event for StartWorkspaceResult

StopWorkspace：Workspace停止请求，公开事件
    id: String--请求ID
    workspace: Entity--目标Workspace Entity
    impl Event for StopWorkspace

StopWorkspaceResult：Workspace停止结果，公开事件
    id: String--原请求ID
    result: Result<Entity, WorkspaceError>--成功时返回已清理的原Workspace Entity
    impl Event for StopWorkspaceResult
```

## 逻辑

```text
事件归属：
    所有插件的事件统一在共享types crate定义，每个插件对应一个事件定义文件
    Workspace使用workspace_events.rs，Agent和AgentImage的事件分别由其对应文件提供
    本crate不重复定义事件，src/events.rs如保留则只重导出共享事件
    删除ReloadWorkspace和ReloadWorkspaceResult，暂不提供reload
    删除StopWorkspaceByReference和对应结果，不保留第二套停止入口

外部事件的最小约定：
    LoadAgentImage与LoadAgentImageResult必须能关联唯一子请求、目标Workspace完整Entity和镜像ResourceID
    AgentImage插件加载或更新指定Entity上的ResourceSet<AgentImage>，写入完成后才发成功结果
    目标Entity已销毁时不得向复用其index的新实体写入；不存在独立镜像Entity返回值
    StartAgent与StartAgentResult必须能关联唯一子请求及成员
    StartAgent至少能表达工作区Entity、成员定义与镜像ResourceID，结果成功返回Agent Entity
    Agent插件从指定工作区的镜像集合取启动材料；若Agent需要长期持有镜像，则保存自身的资源集合条目或所需启动快照，不长期借用Workspace组件
    StopAgent与StopAgentResult必须能关联唯一子请求及目标Agent，结果成功表示目标已完成清理
    外部事件的完整字段和其他插件的实现以后设计，本文件只声明Workspace需要的契约

依赖方向：
    workspace_plugin通过共享types使用事件，不从agent_plugin或agent_image_plugin导入事件
    共享事件涉及的WorkspaceDefinition、ResourceID和错误契约也必须可被共享层引用
    共享types不得反向依赖workspace_plugin来取得WorkspaceError，否则产生循环依赖
```

# types

## 类型

公开：
```text
WorkspaceState：工作区状态机，公开枚举--只表示工作区生命周期及本次操作进度，不跟踪成员运行状态
    Starting(WorkspaceStartState)--成员启动过程尚未完成
    Ready--定义中的全部成员已成功启动，本工作区不再监听其运行状态变化
    Stopping(WorkspaceStopState)--等待成员停止结果
    Failed { error: WorkspaceError }--一次启动或停止已结束但失败，保留尚需清理的工作区

WorkspaceStartState：工作区启动进度，公开类型--由Workspace.state持有，不是独立Resource
    request_id: String--原StartWorkspace请求ID
    pending_images: BTreeMap<String, ResourceID>--镜像加载子请求ID到镜像地址
    image_members: BTreeMap<ResourceID, Vec<ResourceID>>--镜像地址到依赖该镜像的成员Agent地址
    pending_agents: BTreeMap<String, ResourceID>--成员启动子请求ID到成员Agent地址
    error: Option<WorkspaceError>--本次启动首先记录的错误，其余错误记录日志

WorkspaceStopState：工作区停止进度，公开类型--由Workspace.state持有，不是独立Resource
    request_id: String--原StopWorkspace请求ID
    pending_agents: BTreeMap<String, (ResourceID, Entity)>--停止子请求ID到成员Agent地址和完整Entity
    error: Option<WorkspaceError>--本次停止首先记录的错误，其余错误记录日志
```

## 逻辑

```text
命名：
    Starting与StartWorkspace对应，Ready表示工作区启动已完成
    Stopping与StopWorkspace对应，比Closing更直接
    销毁实体就是停止完成，不为已不存在的实体保存Stopped状态

状态转换：
    不存在 -> StartWorkspace -> Starting
    Starting -> 所有子请求成功 -> Ready
    Starting -> 所有子请求结束且有错误 -> Failed
    Ready或Failed -> StopWorkspace -> Stopping
    Stopping -> 所有成员停止成功 -> Entity销毁
    Stopping -> 所有停止子请求结束且有错误 -> Failed

取消旧存储：
    删除WorkspaceAgentState、WorkspaceRegistry和WorkspaceStopReply
    agents不再配套states成员状态表
    异步请求的关联不能凭空消失，因此仅将本次操作需要的等待表放入WorkspaceState
    不保存oneshot回执，不轮询接收器，不另建全局工作区注册表
```

# error

## 类型

公开，共享事件可引用的错误契约，目标定义位于共享types crate而非依赖Workspace插件：
```text
WorkspaceErrorKind：工作区错误分类，公开枚举
    InvalidRequest
    InvalidDefinition
    DuplicateWorkspace
    WorkspaceNotAlive
    WorkspaceBusy
    ResourceResolutionFailed
    AgentImageLoadFailed
    AgentStartFailed
    AgentStopFailed

WorkspaceError：工作区错误，公开结构体
    kind: WorkspaceErrorKind--错误分类
    message: String--错误描述
    new(kind: WorkspaceErrorKind, message: impl Into<String>) -> Self
        构造错误：公开关联函数，保存分类和消息
    kind(&self) -> WorkspaceErrorKind
        读取分类：公开方法
    message(&self) -> &str
        读取消息：公开方法
    impl Clone + Debug + PartialEq + Eq for WorkspaceError
    impl fmt::Display for WorkspaceError
        fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result
            格式化错误：输出分类与消息
    impl std::error::Error for WorkspaceError
```

## 逻辑

```text
错误边界：
    Workspace只包装自己协调的请求与结果错误，不保留VM、Memory、工具或推理初始化的专属错误分类
    外部插件的失败经其结果事件到达，由Workspace映射为镜像加载、成员启动或成员停止失败
    不存在目标的停止记录warning；日志级别不代替结果事件
    src/error.rs如保留则重导出共享错误，不在本插件与共享types之间互相依赖
```

# 后续资源基础设施约定

本节只记录Workspace使用最新资源设计的方式，不修改或完整设计其他插件。

```text
实体组成：
    Workspace Entity
        Workspace { resourceid, definition, project_root, agents, state }
            实现Component和LibraryComponent，是本Entity唯一图书馆组件
        ResourceSet<AgentImage>
            实现Component和ResourcesComponent，以镜像ResourceID为键保存AgentImage条目
    ResourceID只是地址值，不是独立组件
    不存在独立镜像宿主Entity、依赖者集合或资源GC

查询：
    workspace地址通过Workspace::resource_id匹配，受检挂载保证图书馆地址唯一
    image地址通过ResourceSet<AgentImage>::resource_ids匹配，可能查出多个工作区或其他持有者Entity
    已知目标Workspace时直接在该Entity的镜像集合按ID取值，不进行全局任选匹配

生命周期：
    停止全部成员后销毁Workspace Entity，其镜像集合随之释放
    其他Entity上的同ID镜像条目不受影响，磁盘镜像包保留
    不调用release_resource_owner，不使用ResourceOwners，不引入GC System
    Agent插件必须在StopAgentResult成功前完成成员清理，Workspace不能先销毁再让Agent继续借用其镜像集合

依赖方向：
    Workspace通过共享事件契约请求镜像加载，不直接调用AgentImage插件的加载Handler
    如需读取ResourceSet<AgentImage>，AgentImage数据类型必须是可共享的资源契约，而非迫使Workspace依赖镜像插件实现
    该类型的最终定义位置、镜像事件完整字段以及Agent侧取用材料的方式留待对应插件设计
    统一资源插件提供地址、组件trait和类型注册；Workspace不自行实现第二套身份索引

待后续落实：
    三个基础插件的合并与ResourceID、ResourceSet实现
    AgentImage插件向指定Entity加载镜像资源条目的事件契约
    Agent启动和停止事件的完整字段及失败清理规则
    MCL和轮次中止的新入口
    本次不改Rust代码、不恢复reload
```
