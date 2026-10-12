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
types      共享components    Workspace状态机、操作进度与执行能力
error      共享错误契约       错误定义归属见error板块
```

**本文是目标设计，尚未同步到代码。本次只修改Workspace设计文档。**

# lib

## 组件定义位置

Workspace图书馆组件及其实现方法、WorldWorkspaceExt声明与World实现；WorkspaceState等组件公开关联类型随组件契约组织，计划写入共享types crate的`workspace_components.rs`。对应全部事件写入各自的`*_events.rs`，不在插件实现crate重复定义。

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
    execution_entry: Option<WorkspaceExecutionEntry>--工作区专属线程的任务提交入口，私有，不直接向外暴露
    execution_close: Option<WorkspaceExecutionClose>--关闭该线程并等待退出的能力，私有
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
                挂载start_workspace_system、stop_workspace_system和workspace_exception_system
                不插入WorkspaceRegistry
                不通过Agent、AgentImage、Memory、Tool、MCL或LuaRuntime的插件安装标记建立业务插件依赖
```

## 扩展 trait

公开：
```text
WorldWorkspaceExt：工作区世界扩展，公开trait--声明与World实现均放在共享types的workspace_components.rs
    workspace_start(&self, id: impl Into<String>, definition: WorkspaceDefinition)
        请求启动：公开方法，发送StartWorkspace
    workspace_stop(&self, id: impl Into<String>, workspace: Entity) -> Result<(), WorkspaceError>
        请求停止：公开方法，检查Workspace存在且为Running，通过后发送StopWorkspace；其他状态返回WorkspaceNotRunning
        行为：事件Handler仍在受理时重新检查状态，避免检查后到事件处理前状态已改变
    workspace_by_id(&self, id: &ResourceID) -> Result<Entity, WorkspaceError>
        查询工作区：公开方法，要求地址类型为workspace，调用ResourceID::find_entities并确认目标挂载Workspace
        行为：无匹配返回WorkspaceNotAlive，唯一匹配返回Entity，多匹配返回ResourceResolutionFailed，不任取第一个
    workspace_list(&self) -> Vec<Entity>
        查询工作区：公开方法，查询挂载Workspace组件的存活Entity，不读取注册表
    workspace_submit(&self, workspace: Entity, task: WorkspaceExecutionTask) -> Result<(), WorkspaceError>
        提交工作区任务：公开方法
        行为：
            要求目标Workspace存在且state为Running，其他状态拒绝
            通过组件内部execution_entry向该工作区线程提交拥有所有权的任务
            不向调用方返回可长期绕过状态检查的原始提交入口，不持有World借用执行异步任务
            停止受理后不允许新任务入队，已提交任务由所属Agent管理取消与清理
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
    execution_entry和execution_close属于这个工作区实例，一条线程可运行多个Agent VM
    definition只描述初始成员，不是动态成员的上限；空工作区仍保留执行线程
    Running开放外部提交与动态成员启动，Starting仅允许原启动链内部提交，Stopping和Failed不开放新任务
    workspace_of由Agent侧提供，不属于本trait
    World扩展方法统一使用所属领域前缀，Agent为agent_xxx，Workspace为workspace_xxx
    本trait的方法统一以workspace_开头，包括workspace_start、workspace_stop、workspace_submit、workspace_by_id和workspace_list

职责边界：
    Workspace在创建时启动并拥有自己的执行线程，保存入口与关闭能力
    线程内部使用单线程异步执行器调度多个独立Driver VM，创建和调度细节由通用执行设施封装
    Workspace协调成员生命周期，不打开Memory、不组装Lua VM、不执行工具或推理
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
    处理事件：StopWorkspace、StopAgentResult、Result<WorkspaceExecutionCloseOutput, AsyncTaskError>
    产出事件：StopAgent、ClosureRequest、StopWorkspaceResult
    行为：
        克隆本帧全部StopWorkspace，逐个调用handle_stop_workspace
        克隆本帧全部StopAgentResult，逐个调用handle_stop_agent_result
        读取线程关闭闭包结果，调用handle_workspace_execution_closed

workspace_exception_system(world: &mut World)
    工作区运行期异常System：crate公开System
    处理事件：AgentExceptionResult、WorkspaceException
    产出事件：WorkspaceExceptionResult
    行为：分别调用handle_agent_exception_result及handle_workspace_exception；不消费自己的WorkspaceExceptionResult，不将结果重新包装成WorkspaceException
```

## 逻辑

```text
事件驱动：
    System只读取事件并调用Handler，业务处理和事件发送在Handler展开
    没有事件时不扫描工作区判断是否完成
    最后一个必要结果到达时，在该事件的Handler中完成状态转换和结果发送
    启动System保留三类业务输入输出；停止System在原StopWorkspace和StopAgentResult链路外接受线程关闭的异步结果，产出ClosureRequest提交关闭等待
    异步结果不是公开业务通知，只用于非阻塞地等待线程退出
    另设workspace_exception_system只处理运行期成员异常通知，不持续同步成员状态
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
        在创建阶段启动工作区专属执行线程，保存execution_entry和execution_close，不等第一个Agent才创建线程
        线程启动失败且没有成员执行体时回收本次Entity并返回ExecutionStartFailed
        在Starting中保存原始启动请求ID，并按镜像地址归并需要该镜像的成员Agent地址
        为每个不同镜像生成唯一加载子请求ID，在状态中登记关联后发送LoadAgentImage，携带目标Workspace完整Entity和镜像ResourceID
        确认线程提交入口可用后才继续初始成员创建；无成员时置Running并发送StartWorkspaceResult的Ok(workspace)
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
        初始成员只通过原Starting操作关联取得线程提交能力，不向外开放工作区入口；这条内部启动授权不等于允许任意调用者在Starting加入Agent
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
        只有Running接受StopWorkspace；Starting、Stopping和Failed均记录warning并发送WorkspaceNotRunning错误，不启动停止过程
        受理时先将state置为Stopping，关闭外部任务提交门，再保存原请求ID和成员停止关联
        已受理的动态成员加入必须纳入本次操作的等待关系，不能将仍有创建任务的工作区当作无成员工作区
        对已登记成员沿用原StopAgent请求流程，不增加VM、工具或推理清理职责
        为每个成员生成唯一StopAgent子请求ID，在Stopping中登记关联后发送StopAgent
        没有成员且没有已受理的成员创建时直接进入线程关闭步骤，不跳过线程关闭
        不读取Agent内部的VM、工具调用、推理或生命周期字段

handle_stop_agent_result(world: &mut World, event: StopAgentResult)
    处理成员停止结果：crate公开函数
    行为：
        通过结果的子请求ID查找Stopping中等待该结果的Workspace，未匹配时忽略
        按等待记录确认目标成员，消费匹配记录，重复结果不再次处理
        成功时从agents移除对应成员，Agent实体由Agent插件清理，不在此重复despawn
        失败时记录停止错误并保留该成员引用
        调用finish_workspace_stop检查本次停止是否已完成

handle_workspace_execution_closed(world: &mut World, result: Result<WorkspaceExecutionCloseOutput, AsyncTaskError>)
    处理工作区线程关闭结果：crate公开函数
    行为：
        按关闭输出的完整Workspace Entity和停止请求ID检查当前Stopping操作
        成功时确认执行线程已经退出，销毁Workspace并发送StopWorkspaceResult::Ok(原Entity)
        关闭失败时保留组件和关闭能力，置Failed并发送ExecutionCloseFailed错误
        框架错误也必须关联原请求，由提交关闭等待时的回执保障完成错误通知，不能只记录无关联日志
        不在此清理Agent，成员自身清理由Agent插件负责

handle_agent_exception_result(world: &mut World, event: AgentExceptionResult)
    处理成员异常结果：crate公开函数
    行为：按event.workspace地址定位唯一Workspace，核对agents[event.resource_id]与event.agent以及event.instance.owner一致
        成员已替换或工作区不存在时不修改其他实例；exception_id用于一次性处理，重复结果不重复收口
        使用event.result作为Agent层处理结果，不将其Ok理解为原异常不存在，不读取不存在的event.workspace Entity字段
        产WorkspaceExceptionResult，携带workspace/resource_id、agent/agent_resource_id、exception_id及本层处理result
        本层只确认成员归属和记录异常，保留成员、线程及工作区，不自动停止其他成员；清理失败使提交门保持关闭，不伪造已清理事实
        不重新产WorkspaceException，不替代启动/停止结果，不恢复成员状态表

handle_workspace_exception(world: &mut World, event: WorkspaceException)
    处理工作区本层异常：crate公开函数
    行为：核对event.workspace/resource_id及成员身份，按exception_id一次处理；无效关联返回对应处理Err且不修改其他实例
        记录原error并关闭外部提交门；保留工作区、成员及线程，不自动销毁或重启，不强制关闭尚有活动的线程
        发WorkspaceExceptionResult，保留原error并用result表示本层处理成功/失败；必要清理只通过已有成员停止结果链非阻塞协调
        不消费自己的Result，不重新发送Exception，原生命周期请求仍通过StartWorkspaceResult或StopWorkspaceResult收口

```

私有：
```text
finish_workspace_start(world: &mut World, workspace: Entity)
    完成启动判定：私有函数，只由启动事件Handler调用
    行为：
        Starting中还有等待镜像或等待成员请求时返回，不扫描Agent状态
        全部请求结束且没有错误时将state置Running，发送原始id对应的StartWorkspaceResult::Ok(workspace)
        全部请求结束且有错误时将state置Failed，发送原始id对应的StartWorkspaceResult::Err(error)
        失败时保留已创建成员、镜像集合及执行线程，不偷偷销毁仍有执行体的工作区
        Failed不受理普通StopWorkspace，关闭提交门并保留成员及线程；异常结果只确认/记录失败，不隐式恢复Running或强制销毁
        转移状态后不再保留本次启动等待记录，原请求只回复一次

finish_workspace_stop(world: &mut World, workspace: Entity)
    完成停止判定：私有函数，只由停止事件Handler调用
    行为：
        Stopping中还有等待成员请求时返回
        全部成员请求结束且没有错误时调用execution_close关闭工作区执行线程，提交异步闭包等待退出；关闭等待只提交一次
        不阻塞ECS主线程，不先销毁Workspace，不让线程在成员清理前退出
        线程退出结果由handle_workspace_execution_closed完成实体回收和回复
        成员停止失败时置Failed并保留线程及未清理成员，发送StopWorkspaceResult::Err，不强制关闭其执行线程
        Workspace Entity销毁时，其Workspace组件与ResourceSet<AgentImage>一同释放
        不调用release_resource_owner，不登记ResourceOwners，不等待GC，也不删除磁盘镜像包
```

## 逻辑

```text
启动：
    StartWorkspace -> Starting -> 创建工作区执行线程并保存入口和关闭能力 -> LoadAgentImage
    LoadAgentImageResult -> 确认本Entity镜像集合已更新 -> StartAgent
    StartAgentResult -> 更新agents -> 全部成功后Running -> StartWorkspaceResult
    同一工作区的多个成员使用同一镜像时只提交一次加载，加载完成后分别启动成员
    不同工作区可以各自保存同ID镜像条目，一方加载更新不会自动覆盖另一方集合
    StartWorkspaceResult成功意味着定义中的全部成员都取得StartAgentResult成功结果，不再是仅创建了Workspace Entity
    StartAgentResult成功的准确边界由Agent插件保证，Workspace不额外监听Agent初始化事件

停止：
    StopWorkspace -> Stopping -> StopAgent
    StopAgentResult -> 更新成员集合 -> 全部成功后关闭工作区线程 -> 等待线程退出 -> 销毁Workspace -> StopWorkspaceResult
    多成员工作区必须等待所有已发StopAgent请求结束，不能收到第一个结果就销毁Workspace
    StopAgentResult成功必须表示Agent已完成清理，不能仅表示接受了停止请求
    工具、推理、VM如何停止由Agent插件负责，Workspace不以轮询代替完成事件
    成功结果中的Entity表示已清理的实例身份，不再保证能查询其组件

过程关联：
    顶层请求id与子请求id分开，子请求id在运行期唯一，不只用成员名或可复用的实体index构造
    不使用全局WorkspaceRegistry，以Workspace.state中的等待表匹配结果
    可在结果事件到达时查询Workspace组件寻找匹配的子请求，不增加每帧轮询System
    一条等待记录只消费一次，旧结果不能推进新实例的操作

异常与失败边界：
    只有Running受理停止事件和开放提交入口；Starting、Stopping、Failed均拒绝，未设计启动中取消协议
    启动或停止失败返回Err并保留需要管理的实例和线程；Failed关闭提交门且不接受普通Stop，异常处理不得隐式恢复或强制回收
    不存在的Workspace记录warning，同时返回Err完成请求关联
    本契约不提供启动中停止、失败自动回滚或停止幂等成功，不以异常结果替代必要生命周期回执
    未收到必要结果时保持对应状态，本次不引入超时或强制销毁策略
```

# events

## 定义位置

所有事件无论对外业务通知还是内部调度，都定义在共享types的`workspace_events.rs`，插件实现不再有`events.rs`。内部仅表示用途，不意味着在共享crate中使用pub(crate)后其他crate仍能访问；供插件使用的事件需要公开可见。

自动异步响应仍是框架的`Result<T, E>`，不另造包装事件；其中具名输出、错误及其传递所需的关联契约也在共享事件文件定义，不使共享types反向依赖插件私有类型。Plugin配置和非事件处理状态仍留在实现侧。基础设施事件如ClosureRequest沿用基础设施定义，不在业务文件复制。

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

WorkspaceException：工作区本层运行期异常输入，公开事件
    exception_id: String--本次异常唯一ID
    workspace: Entity--完整工作区实例
    resource_id: ResourceID--工作区逻辑地址
    agent: Entity--异常成员完整Entity
    agent_resource_id: ResourceID--异常成员逻辑地址
    error: WorkspaceError--工作区层异常详情
    impl Event for WorkspaceException

WorkspaceExceptionResult：工作区异常处理结果，公开事件
    exception_id: String--原异常ID，Agent来源沿用同链异常ID
    workspace: Entity--完整工作区实例
    resource_id: ResourceID--工作区逻辑地址
    agent: Entity--异常成员完整Entity
    agent_resource_id: ResourceID--异常成员逻辑地址
    error: Option<WorkspaceError>--本层原异常，Agent来源如原详情不可用则不虚构
    result: Result<(), WorkspaceError>--本层处理成功/失败，不表示原异常不存在或成员已清理
    impl Event for WorkspaceExceptionResult
```

## 逻辑

```text
事件归属：
    所有插件的事件统一在共享types crate定义，每个插件对应一个事件定义文件
    Workspace使用workspace_events.rs，Agent和AgentImage的事件分别由其对应文件提供
    本crate不定义事件，也不保留src/events.rs；从共享types直接使用事件
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
    AgentExceptionResult使用exception_id/agent/resource_id/workspace(ResourceID)/instance/error/result/execution_ended/cleanup_completed；不得沿用AgentException旧直达路由
    Driver异常到Agent结果、Agent结果到Workspace结果均单向上行，每层Exception输入由本层独占处理，Result不回投为Exception

依赖方向：
    workspace_plugin通过共享types使用事件，不从agent_plugin或agent_image_plugin导入事件
    共享事件涉及的WorkspaceDefinition、ResourceID和错误契约也必须可被共享层引用
    共享types不得反向依赖workspace_plugin来取得WorkspaceError，否则产生循环依赖
```

## 具名异步载荷

共享types的workspace_events.rs中的内部结果契约：
```text
WorkspaceExecutionCloseOutput：线程关闭等待结果，公开共享类型--只用于异步结果通道
    workspace: Entity--原工作区完整Entity
    request_id: String--原StopWorkspace请求ID
    result: Result<(), WorkspaceError>--线程关闭完成或失败
```


# types

## 类型

公开：
```text
WorkspaceState：工作区状态机，公开枚举--只表示工作区生命周期及本次操作进度，不跟踪成员运行状态
    Starting(WorkspaceStartState)--成员启动过程尚未完成
    Running--线程可用且初始成员启动成功，开放提交入口和停止受理，允许成员动态加入或退出
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
    closing_execution: bool--是否已经提交线程关闭等待，防止重复提交
    error: Option<WorkspaceError>--本次停止首先记录的错误，其余错误记录日志
```

公开共享契约，声明和实现位于共享workspace_components.rs，具体线程执行由通用设施提供：
```text
WorkspaceLocalTask：线程内任务，公开类型别名
    Pin<Box<dyn Future<Output = ()> + 'static>>
    说明：不要求Send，只在工作区线程内创建、调度和释放

WorkspaceExecutionTask：任务创建闭包，公开类型别名--跨线程传递的是创建能力，不是Lua VM或本地Future
    Box<dyn FnOnce() -> WorkspaceLocalTask + Send + 'static>
    说明：闭包捕获拥有所有权且可跨线程的启动数据，不捕获World或组件借用
        工作区线程调用闭包后才创建Future及Lua VM

WorkspaceExecutionEntry：工作区线程提交入口，公开类型--内部Sender不公开，不绕过Running受检入口
    sender: mpsc::UnboundedSender<WorkspaceExecutionCommand>--跨线程命令发送端
    admission: Arc<Mutex<WorkspaceAdmission>>--各入口克隆共用的提交门
    说明：通过workspace_submit进行外部提交；初始成员使用Starting操作授权的内部提交路径
        检查提交门与入队必须在同一短期锁内完成，停止时关闭提交门也使用该锁
        锁不跨await，保证关闭门后不能通过旧入口克隆继续提交

WorkspaceExecutionClose：工作区线程关闭能力，公开类型
    close(&self) -> impl Future<Output = Result<(), WorkspaceError>>
        关闭线程：公开异步方法
        行为：
            关闭提交门，发送一次WorkspaceExecutionCommand::Close
            在线程外等待线程退出；std::thread::JoinHandle::join属于阻塞等待，不能直接在ECS主线程或通用异步执行线程上调用
            通过阻塞等待设施完成join，线程panic或关闭失败转换为ExecutionCloseFailed
        前置条件：全部Agent Driver、execution的Inference/Sandbox/Other活动及MCL队列均收口，已受理创建任务已收口；关闭线程不是取消所有Agent的替代品
        说明：线程JoinHandle只能消费一次，关闭能力内部保证不重复join；具体共享封装实现时确定

WorkspaceExecutionCommand：工作区线程命令，公开共享内部契约
    Spawn(WorkspaceExecutionTask)--在目标线程内创建并启动任务
    Close--停止接收命令，等待已有任务结束并退出线程

WorkspaceAdmission：提交门，公开共享内部类型
    Starting--仅允许初始启动操作的授权提交
    Running--允许受检外部提交
    Closed--拒绝所有新任务，Stopping或Failed时使用
```


## 逻辑

```text
工作区执行线程：
    创建工作区时建立命令通道并启动一条std线程，在线程内创建Tokio current_thread runtime和LocalSet
    将提交Sender、提交门和关闭能力保存在Workspace中，Receiver由线程拥有
    使用runtime.block_on(local.run_until(...))运行命令循环
    循环同时接收命令和收集本地任务的结束结果
        Spawn(factory)：在线程内调用factory得到WorkspaceLocalTask，通过JoinSet::spawn_local加入任务集合
        本地任务结束：join_next取得结果，异常报告通过捕获的RuntimeEventSender发送，不访问World
        Close或命令通道关闭：关闭Receiver，不再创建任务，丢弃尚未处理的创建请求时也必须完成对应失败通知
    退出命令循环后继续等待已受理任务结束，任务集合为空才释放执行器并返回线程函数
    正常关闭不abort所有任务，不以丢弃LocalSet代替Agent自己的清理
    JoinSet用于拥有和回收线程内任务，不是全局Agent注册表

异步与线程边界：
    Task创建闭包必须Send，本地Future不必Send；Lua VM在工作区线程内创建，不跨线程搬运
    spawn_local不创建操作系统线程，同工作区任务在await时让出，不同工作区线程可并行
    Driver宿主wait使用异步等待，不在本地执行任务中block_on宿主Future
    同步阻塞工作必须移出该线程，纯Lua长循环必须通过执行限制或让出机制防止阻塞全部本地任务
    简单select循环不提供Lua指令抢占或公平调度保证，不将async误写为自动抢占
    RuntimeEventSender可被任务捕获跨线程发送完成或异常事件，组件修改仍由ECS主线程受理事件完成

命名：
    Starting表示工作区线程和初始成员尚在启动，Running表示已完成启动且入口开放
    Stopping与StopWorkspace对应，比Closing更直接
    销毁实体就是停止完成，不为已不存在的实体保存Stopped状态

状态转换：
    不存在 -> StartWorkspace -> Starting
    Starting -> 所有子请求成功 -> Running
    Starting -> 所有子请求结束且有错误 -> Failed
    Running -> StopWorkspace -> Stopping
    Starting、Stopping或Failed -> StopWorkspace -> 错误结果，状态不变
    Stopping -> 所有成员停止成功 -> 关闭线程并等待退出 -> Entity销毁
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
    WorkspaceNotRunning
    ExecutionStartFailed
    ExecutionCloseFailed

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

## 资源基础设施约定

本节只记录Workspace使用最新资源设计的方式，不修改或完整设计其他插件。

```text
实体组成：
    Workspace Entity
        Workspace { resourceid, definition, project_root, agents, state, execution_entry, execution_close }
            实现Component和LibraryComponent，是本Entity唯一图书馆组件
        ResourceSet<AgentImage>
            实现Component和ResourcesComponent，以镜像ResourceID为键保存AgentImage条目
    ResourceID只是地址值，不是独立组件
    不存在独立镜像宿主Entity、依赖者集合或资源GC
    已被克隆的执行入口也必须遵守同一提交门；Stopping不能只靠隐藏字段阻止旧句柄继续提交

查询：
    workspace地址通过Workspace::resource_id匹配，受检挂载保证图书馆地址唯一
    image地址通过ResourceSet<AgentImage>::resource_ids匹配，可能查出多个工作区或其他持有者Entity
    已知目标Workspace时直接在该Entity的镜像集合按ID取值，不进行全局任选匹配

生命周期：
    只有Running接受停止，先关闭外部入口，成员由Agent自己停止，随后关闭工作区线程并等待退出
    最后销毁Workspace Entity，其镜像集合随之释放
    其他Entity上的同ID镜像条目不受影响，磁盘镜像包保留
    不调用release_resource_owner，不使用ResourceOwners，不引入GC System
    Agent插件必须在StopAgentResult成功前完成成员清理，Workspace不能先销毁再让Agent继续借用其镜像集合

依赖方向：
    Workspace通过共享事件契约请求镜像加载，不直接调用AgentImage插件的加载Handler
    如需读取ResourceSet<AgentImage>，AgentImage数据类型必须是可共享的资源契约，而非迫使Workspace依赖镜像插件实现
    AgentImage是共享资源契约，镜像加载事件在共享agent_image_events.rs；Agent从指定Workspace Entity按image ID取得启动快照
    统一资源插件提供地址、组件trait和类型注册；Workspace不自行实现第二套身份索引

实现边界：
    本文明确共享components与events归属以及成员完整停止证明；具体Rust实现不在本次修改范围
    MCL由其队列及AgentExecutionScope完成取消和回执收口，Workspace不路由旧AgentMessage事件
    本次不改Rust代码、不恢复reload
```
