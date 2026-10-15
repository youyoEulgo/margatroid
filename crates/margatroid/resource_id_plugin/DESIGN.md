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

本文按 `lib`、`system`、`handler`、`events`、`types`、`error` 六个板块组织。

```text
lib        src/lib.rs        统一插件、组件trait和注册入口
system     src/system.rs     当前目标设计无System
handler    src/handler.rs    类型注册、组件挂载和地址查询
events     共享types crate    当前目标设计无事件，不保留插件events.rs
types      src/types.rs      ResourceID、ResourceSet和类型注册表
error      src/error.rs      解析、注册、挂载和查询错误
```

**统一资源基础设施已实现。** 本crate继续使用resource_id_plugin名称，对外安装ResourcePlugin，合并旧resource_library_plugin和library_component_plugin职责；旧两个crate及其文档已移除，不再是workspace成员。ResourceID、集合、trait、注册与查询基础实现均在本crate，且仅依赖core_plugin和serde，可被共享types直接依赖而无业务反向依赖。

# lib

## 组件定义位置

领域图书馆组件和资源条目由共享types提供；五类条目目前组合在resource_components.rs，Agent、Workspace、Client分别有对应组件模块，本基础设施不依赖这些领域定义。

ResourceSet与组件trait实际位于本crate的components.rs；ResourceID在types.rs；ResourceTypeRegistry及受检注册、查询和WorldResourceExt在handler.rs。本crate作为纯基础层不依赖业务types，Plugin只安装资源和注册表，因此共享types使用这些契约不会形成循环。

ResourceID等基础值与集合契约统一归共享基础类型层；共享types不能反向依赖资源插件实现。公开trait的关联类型及方法签名只能引用公开可见契约，不引用插件私有类型。

## 类型

公开：
```text
ResourcePlugin：统一资源管理插件，公开单元结构体--提供地址值、组件分类、类型注册和实体查询
    impl Plugin for ResourcePlugin
        Plugin：公开trait实现
        build(self, app: &mut App)
            安装插件：公开trait方法
            行为：
                重复安装时拒绝
                插入ResourcePluginInstalled和空ResourceTypeRegistry
                不创建独立的资源宿主Entity
                不注册GC、依赖者扫描或回收System

ResourcePluginInstalled：统一资源插件安装标记，公开Resource
    impl Resource for ResourcePluginInstalled

LibraryComponent：图书馆组件，公开trait--描述实体自身的领域身份
    约束：Component
    resource_id(&self) -> &ResourceID
        读取自身地址：公开trait方法，返回该图书馆组件保存的ResourceID

ResourceInner：资源条目契约，公开trait--统一读取条目的本地别名，不保存第二份ID
    约束：Send + Sync + 'static
    get_alias(&self) -> &str
        读取别名：公开trait方法，返回资源条目的alias

ResourcesComponent：资源集合组件，公开trait--扩展已有集合契约，不另建重复的资源查询trait
    约束：Component
    type Inner: Send + Sync + 'static--集合的具体资源条目类型；不强制镜像等既有条目实现ResourceInner
    list_id(&self) -> impl Iterator<Item = &ResourceID> + '_
        列出多个资源ID：公开trait方法，返回集合的全部ID，而非单个ID
        约束：Self::Inner: ResourceInner
    get_by_id(&self, id: &ResourceID) -> Option<&Self::Inner>
        按ID读取条目：公开trait方法，命中时借用条目，不存在时返回None
        约束：Self::Inner: ResourceInner
    resource_ids(&self) -> impl Iterator<Item = &ResourceID> + '_
        兼容基础查询：公开trait方法，不要求ResourceInner，保留注册、挂载校验和实体发现使用的接口
```

## 扩展 trait

公开：
```text
WorldResourceExt：资源管理世界扩展，公开trait--定义及纯查询/注册/受检挂载实现位于本基础crate的handler.rs，不依赖业务插件
    register_library<C>(&mut self, resource_type: &str) -> Result<(), ResourceError>
        注册图书馆类型：公开方法，建立地址类型到具体图书馆组件类型的对应关系
        约束：C: LibraryComponent
        行为：调用register_library_type::<C>
    register_resources<C>(&mut self, resource_type: &str) -> Result<(), ResourceError>
        注册资源集合类型：公开方法，建立地址类型到具体资源集合组件类型的对应关系
        约束：C: ResourcesComponent
        行为：调用register_resources_type::<C>
    insert_library<C>(&mut self, entity: Entity, component: C) -> Result<(), ResourceError>
        挂载图书馆组件：公开方法，通过统一入口检查领域身份互斥与地址一致性
        约束：C: LibraryComponent
        行为：调用insert_library_component::<C>
    insert_resources<C>(&mut self, entity: Entity, component: C) -> Result<(), ResourceError>
        挂载资源集合：公开方法，检查集合类型与内部地址类型一致
        约束：C: ResourcesComponent
        行为：调用insert_resources_component::<C>
    impl WorldResourceExt for World
        World：公开trait实现，按共享基础层中的上述纯处理函数实现；插件安装只调用这些共享入口
```

## 逻辑

```text
组件分类：
    一个领域Entity恰好有一个LibraryComponent，可以同时有多个不同具体类型的ResourcesComponent
    Agent、Workspace、Client属于图书馆组件，各自在组件内部保存自身ResourceID
    ToolResource、PromptResource、SandboxResource和AgentImage属于资源条目
    资源条目通过ResourceSet<T>挂载，不把这些条目本身标为LibraryComponent
    AgentImage明确属于资源类，不再作为独立图书馆实体，不另挂旧ResourceId组件

MCL当前分类（不改变基础集合分类）：
    IMPORT TOOL仅接受tool，IMPORT MESSAGE接受prompt，IMPORT SANDBOX接受sandbox
    TOOL/MESSAGE/SANDBOX是MCL值能力分类，分别解析tool、prompt、sandbox资源类型和具体ResourceSet
    Skill与Hook资源类型、资源插件及专用执行入口已移除，指导正文也由普通Tool返回
    RESOURCE不再是目标Block字段类型，但ResourcesComponent和ResourceID基础概念不删除或改名
    Agent.import_bindings是alias命名权威，受检导入跨集合检查冲突；本插件不新增全局alias表
    CallableResource与SandboxValue携带所属ID及不可变共享快照，旧Block值不因集合重载自动变更
    snapshot的resource_id是来源关联，不要求资源条目T重复保存map key；二者职责不同

trait的能力边界：
    LibraryComponent要求返回自身ID，但Rust trait不能强制某个具体字段名称
    具体图书馆组件按约定包含resource_id: ResourceID字段
    ResourcesComponent通过list_id列出集合内部ID，get_by_id按ID返回具体条目；resource_ids兼容基础查询
    ResourceSet<T>通过map的键提供ID，五类资源条目T: ResourceInner通过get_alias返回alias；集合存储基本约束保持Send + Sync + 'static，不重复在T中存ID
    两个trait不是互斥机制，单个Entity只能有一个图书馆组件的检查由统一挂载入口完成

ECS挂载边界：
    ResourceSet<ToolResource>与ResourceSet<AgentImage>是不同具体组件类型，可以共存
    再次插入同一具体组件类型遵循ECS语义，替换整份旧组件，不自动合并集合
    只更新一个资源条目时修改集合的对应键，不重新插入整份集合
    基础ECS的insert_component仍可能绕过检查；实现时所有领域图书馆挂载必须收口到受检入口
    本设计不声称trait本身能阻止底层任意插入，也不声称仅凭trait能强制每个裸Entity都有图书馆组件
```

# system

## 函数

```text
无System。
```

## 逻辑

```text
资源内容随所属Entity的组件一起销毁，不引入ResourceOwners、引用计数或手写GC
本插件不负责停止Agent的后台执行；拥有实体的领域插件在适当时机销毁自身实体
查询由调用者按需执行，不定期扫描生成另一份实体地址索引
```

# handler

本板块函数及泛型辅助实现位于本基础crate的handler.rs。此crate作为共享基础层不依赖业务插件；Plugin仅负责安装资源，业务插件通过受检World入口注册领域类型。

## 函数

crate公开：
```text
register_library_type<C>(world: &mut World, resource_type: &str) -> Result<(), ResourceError>
    注册图书馆组件类型：crate公开函数
    约束：C: LibraryComponent
    行为：
        检查插件安装状态和resource_type格式
        将resource_type、TypeId::of::<C>()、Library分类及query_library::<C>、has_component::<C>函数指针登记到注册表
        同一地址类型、具体组件类型和分类重复注册时幂等成功
        同一地址类型注册到不同组件或分类时返回TypeRegistrationConflict
        同一组件类型绑定不同地址类型或不同分类时也拒绝

register_resources_type<C>(world: &mut World, resource_type: &str) -> Result<(), ResourceError>
    注册资源集合组件类型：crate公开函数
    约束：C: ResourcesComponent
    行为：
        检查插件安装状态和resource_type格式
        登记resource_type、TypeId::of::<C>()、Resources分类及query_resources::<C>、has_component::<C>函数指针
        重复与冲突规则同register_library_type

insert_library_component<C>(world: &mut World, entity: Entity, component: C) -> Result<(), ResourceError>
    挂载图书馆组件：crate公开函数
    约束：C: LibraryComponent
    行为：
        要求Entity存活、插件已安装、C已注册为Library分类
        要求component.resource_id()的类型段与C注册的地址类型一致
        检查注册表中其他图书馆类型是否已经挂在目标Entity，存在时返回LibraryConflict
        检查同一图书馆地址是否已被其他Entity使用，存在时返回LibraryAddressOccupied
        验证全部通过后插入组件，同类型组件替换仍需通过以上检查
        失败时不改变原有组件

insert_resources_component<C>(world: &mut World, entity: Entity, component: C) -> Result<(), ResourceError>
    挂载资源集合：crate公开函数
    约束：C: ResourcesComponent
    行为：
        要求Entity存活、插件已安装、C已注册为Resources分类
        遍历component.resource_ids()，校验每个ID类型段与C注册的地址类型一致
        校验集合列出的ID没有重复；ResourceSet<T>由map键天然保证这一点
        验证全部通过后插入组件
        不检查其他Entity是否拥有相同资源地址，同一资源地址允许出现在多个Entity上
        失败时不改变原有组件

find_resource_entities(world: &World, id: &ResourceID) -> Result<Vec<Entity>, ResourceError>
    查询包含地址的实体：共享基础层公开纯查询函数，定义及实现于本基础crate的handler.rs
    行为：
        插件或注册表缺失时返回PluginMissing
        按id.resource_type()查找注册项，未注册时返回TypeNotRegistered
        调用注册项的query函数，在对应具体组件类型中匹配完整ID
        去重并按完整Entity稳定排序后返回
        类型已注册但没有匹配实体时返回空数组，不把多结果当作错误
```

私有：
```text
query_library<C>(world: &World, id: &ResourceID) -> Vec<Entity>
    查询图书馆地址：私有泛型函数
    约束：C: LibraryComponent
    行为：
        遍历挂载C的存活Entity
        比较C::resource_id()与id，匹配则记录Entity
        不在找到第一个结果后停止，即使底层越过受检入口产生重复也返回全部实际匹配者

query_resources<C>(world: &World, id: &ResourceID) -> Vec<Entity>
    查询集合内地址：私有泛型函数
    约束：C: ResourcesComponent
    行为：
        遍历挂载C的存活Entity
        调用resource_ids()，存在完整ID匹配时记录该Entity一次
        返回所有包含目标资源的Entity

has_component<C>(world: &World, entity: Entity) -> bool
    检查组件存在：私有泛型函数
    约束：C: Component
    行为：返回目标Entity是否挂载C

validate_type(value: &str) -> Result<(), ResourceIDError>
    校验类型段：私有函数
    行为：要求非空，仅包含小写字母、数字、下划线或连字符

validate_part(value: &str, error: ResourceIDError) -> Result<(), ResourceIDError>
    校验作用域或名称：私有函数
    行为：要求非空、不是单独的点或双点、不含控制字符、斜杠、反斜杠或冒号

validate_tag(value: &str) -> Result<(), ResourceIDError>
    校验标签：私有函数
    行为：要求非空，首字符为字母数字，后续只允许字母数字、下划线、连字符或点
```

## 逻辑

```text
注册与查询：
    业务插件在安装时用具体组件类型注册自己的地址类型
    tool -> ResourceSet<ToolResource>
    prompt -> ResourceSet<PromptResource>
    sandbox -> ResourceSet<SandboxResource>
    image -> ResourceSet<AgentImage>
    agent -> Agent
    workspace -> Workspace
    client -> Client
    本插件不知道这些领域类型的字段和执行逻辑，映射由各领域插件提供

类型擦除：
    TypeId只表示类型身份，不能单独用于运行时调用query_with::<C>
    泛型注册入口为C生成查询函数指针，注册表保存该函数指针供运行时调用
    不要求LibraryComponent或ResourcesComponent成为dyn对象
    不为查找地址复制资源内容，不维护ResourceID到Entity列表的第二份索引

唯一性：
    图书馆组件的自身地址通过受检挂载入口保持唯一，资源集合中的地址允许跨Entity重复
    公共ResourceID查询统一返回数组，调用方不可默认结果只有一个
    按ID查询得到的是包含地址的Entity，不是需要另外创建的资源宿主Entity
```

# events

## 类型

```text
本插件不定义事件。
```

## 逻辑

```text
组件注册、挂载与查询是同步API，不需要事件中转
业务插件的加载、导入、启动和停止事件由共享types crate定义，本插件不代替它们制定流程
ResourceID归共享基础类型层，资源插件实现依赖该共享契约；共享业务事件不反向依赖本插件或具体业务插件实现
```

# types

## 类型

公开：
```text
ResourceID：资源地址值，公开结构体--不实现Component，可用于字段、集合键和事件载荷
    resource_type: String--地址类型段，私有字段
    scope: String--作用域，私有字段
    name: String--资源名称，私有字段
    tag: String--标签，私有字段
    parse(value: impl AsRef<str>) -> Result<Self, ResourceIDError>
        解析地址：公开关联函数，调用FromStr
    new(resource_type: impl Into<String>, scope: impl Into<String>, name: impl Into<String>, tag: Option<String>) -> Result<Self, ResourceIDError>
        构造地址：公开关联函数，校验全部字段，缺省tag为latest
    resource_type(&self) -> &str
        读取类型：公开方法，返回类型段
    scope(&self) -> &str
        读取作用域：公开方法
    name(&self) -> &str
        读取名称：公开方法
    tag(&self) -> &str
        读取标签：公开方法
    find_entities(&self, world: &World) -> Result<Vec<Entity>, ResourceError>
        查询包含者：公开方法，调用本基础crate的find_resource_entities(world, self)，不依赖业务Handler
    impl Clone + Debug + PartialEq + Eq + PartialOrd + Ord + Hash for ResourceID
    impl FromStr for ResourceID
        from_str(value: &str) -> Result<Self, ResourceIDError>
            解析字符串：接受type:scope/name[:tag]，省略tag时为latest，拒绝格式错误
    impl fmt::Display for ResourceID
        fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result
            格式化地址：输出type:scope/name:tag
    impl Serialize for ResourceID
        序列化：输出规范化地址字符串
    impl Deserialize for ResourceID
        反序列化：从字符串解析并校验

ResourceSet<T>：同类资源集合，公开泛型结构体--作为一个组件挂在使用它的Entity上
    约束：T: Send + Sync + 'static
    entries: BTreeMap<ResourceID, T>--资源地址到条目，私有字段，ID只存于键中
    new() -> Self
        构造空集合：公开关联函数
    insert(&mut self, id: ResourceID, value: T) -> Option<T>
        写入条目：公开方法，同ID更新并返回旧值，无旧值时返回None
    list_id(&self) -> impl Iterator<Item = &ResourceID> + '_
        列出多个ID：公开方法，返回entries的全部键
        约束：T: ResourceInner
    get_by_id(&self, id: &ResourceID) -> Option<&T>
        读取条目：公开方法，按map中的ID键取值
        约束：T: ResourceInner
    get(&self, id: &ResourceID) -> Option<&T>
        兼容读取条目：公开方法，直接读取entries，不要求ResourceInner
    get_mut(&mut self, id: &ResourceID) -> Option<&mut T>
        修改条目：公开方法，按ID取可变条目；不提供map键的可变引用
    remove(&mut self, id: &ResourceID) -> Option<T>
        移除条目：公开方法，按ID移除并返回原值
    iter(&self) -> impl Iterator<Item = (&ResourceID, &T)> + '_
        遍历条目：公开方法，按键升序遍历集合中的ID与内容
        说明：使用BTreeMap以获得稳定的遍历顺序，遍历结果按ResourceID的Ord顺序排列
            排序按类型、作用域、名称、标签依次比较，tag不按语义版本排序
            单个Entity的资源条目数量有限，O(log n)查找的代价低于稳定输出带来的可读性
    impl Default for ResourceSet<T>
        default() -> Self
            构造默认集合：返回空集合，不要求T实现Default
    impl Component for ResourceSet<T>
        Component：公开泛型trait实现
        约束：T: Send + Sync + 'static
    impl ResourcesComponent for ResourceSet<T>
        ResourcesComponent：公开泛型trait实现
        约束：T: Send + Sync + 'static
        type Inner = T
        list_id(&self) -> impl Iterator<Item = &ResourceID> + '_
            列出地址：返回entries的键迭代器
            约束：T: ResourceInner
        get_by_id(&self, id: &ResourceID) -> Option<&T>
            读取条目：返回entries.get(id)
            约束：T: ResourceInner
        resource_ids(&self) -> impl Iterator<Item = &ResourceID> + '_
            兼容基础查询：直接返回entries.keys()，不要求ResourceInner
```

crate公开：
```text
ResourceTypeRegistry：资源类型注册表，公开共享Resource--定义于本基础crate的handler.rs--只保存类型关系和函数，不保存实例地址索引
    registrations: HashMap<String, ResourceTypeRegistration>--地址类型段到具体组件注册项
    impl Resource for ResourceTypeRegistry

ResourceTypeRegistration：具体组件注册项，公开共享结构体--定义于本基础crate的handler.rs
    component_type: TypeId--具体组件类型
    kind: ResourceComponentKind--图书馆或资源集合分类
    query: fn(&World, &ResourceID) -> Vec<Entity>--针对已注册具体类型生成的查询函数
    contains: fn(&World, Entity) -> bool--检查目标是否挂载该具体组件

ResourceComponentKind：组件分类，公开共享枚举--定义于本基础crate的handler.rs
    Library
    Resources
```

## 逻辑

```text
存储示例：
    Agent A Entity
        Agent { resource_id: agent:demo/a:latest }
        ResourceSet<ToolResource>
            tool:local/bash:latest -> ToolResource { alias, description, input_schema, script_path }
            tool:local/review:latest -> ToolResource
        ResourceSet<AgentImage>
            image:local/coder:latest -> AgentImage
    Agent B Entity
        Agent { resource_id: agent:demo/b:latest }
        ResourceSet<ToolResource>
            tool:local/bash:latest -> ToolResource { alias, description, input_schema, script_path }
    查询tool:local/bash:latest返回Agent A和Agent B的Entity
    查询image:local/coder:latest返回所有挂载对应镜像条目的Entity，而不是独立镜像实体
    示例不规定只能由Agent保存镜像，Workspace也可挂载ResourceSet<AgentImage>

数据唯一来源：
    每个Entity自己的ResourceSet<T>是该Entity使用该类资源的数据来源
    不再要求同ID运行时内容在所有Entity间共享唯一实例
    磁盘包可以仍位于全局路径，与每个Entity拥有自己的运行时条目不冲突
    重复IMPORT不因同ID已存在而跳过，仍重新加载；成功仅替换目标集合对应ID键，不自动覆盖其他Entity的条目
    合理安全补充：读取、校验或挂载失败保留原条目并返回失败，不能把旧条目当作本次加载成功
    IMPORT的Agent许可由调用方按Agent.info.image_dependencies完整ID精确匹配，无builtin例外；基础集合及加载插件不重复执行该领域许可检查
    alias属于所在Entity的本地使用名称，不属于全局ResourceID；消费方准备可见集合时不同ID的alias冲突必须失败，Inference与tool_call一致
    一份集合内一个ID对应一个条目；需要同一资源多个别名时须另行设计别名表，不用重复map键表示

使用与查询：
    全局find_entities用于发现哪些Entity包含资源
    执行Agent A的工具时直接在A的ResourceSet<ToolResource>里按ID读取，不能取全局查询的第一个Entity执行
    查询结果是查询时的快照，不是持有令牌，也不延长实体生命周期
    替换或销毁组件后，下一次查询以当前World为准

校验边界：
    ResourceSet<T>不自带地址类型注册信息，insert只维护map，不负责与World注册表对照
    受检挂载会校验内部ID；挂载后的增量导入也必须由领域写入入口校验ID类型，不能把任意类型ID塞进已挂载集合
    图书馆ID同样不应通过任意字段修改绕过地址唯一性检查
    这些约束不靠额外身份副本或GC维持，后续实现需收紧领域写入API

组件实现归属：
    ResourceSet<T>与ResourceInner、ResourcesComponent实际定义于本基础crate的components.rs，由定义侧实现Component及集合trait
    业务crate不再分别为ResourceSet<ToolResource>等具体实例重复实现Component
    ToolResource等条目的alias与ResourceInner实现由共享领域components文件定义，本基础设施不依赖具体领域类型
    AgentImage本次不改，不新增alias或ResourceInner实现；ResourceSet的基本存储、get和resource_ids不要求ResourceInner，保持既有镜像集合兼容
    ResourceInner约束只用于新增条目查询契约的方法，不扩散到集合的Component实现、注册及基础查询
```

# error

## 类型

公开：
```text
ResourceIDError：资源地址解析错误，公开枚举
    Empty
    InvalidType
    InvalidScope
    InvalidName
    InvalidTag
    InvalidFormat
    impl Clone + Debug + PartialEq + Eq for ResourceIDError
    impl fmt::Display for ResourceIDError
        格式化错误：输出地址解析失败原因
    impl std::error::Error for ResourceIDError

ResourceError：资源管理错误，公开枚举
    PluginMissing
    TypeNotRegistered { resource_type: String }
    InvalidResourceType { resource_type: String }
    TypeRegistrationConflict { resource_type: String }
    EntityNotAlive { entity: Entity }
    ComponentNotRegistered { component_type: TypeId }
    ComponentKindMismatch { component_type: TypeId }
    AddressTypeMismatch { id: ResourceID, expected: String }
    DuplicateEntryID { id: ResourceID }
    LibraryConflict { entity: Entity }
    LibraryAddressOccupied { id: ResourceID, entities: Vec<Entity> }
    impl Debug for ResourceError
    impl fmt::Display for ResourceError
        格式化错误：输出类别和相关地址、实体或类型信息
    impl std::error::Error for ResourceError
```

## 逻辑

```text
查询结果：
    没有匹配实体返回Ok空数组，多匹配返回Ok实体数组
    不沿用旧唯一查询接口的Missing和Duplicate错误语义
    LibraryAddressOccupied只用于受检图书馆挂载时的地址占用检查，不用于禁止跨Entity共享资源地址

迁移范围：
    删除ResourceRef，以ResourceID统一地址值
    删除独立ResourceId组件，把图书馆自身ID移入具体图书馆组件
    删除ResourceOwners、install_resource创建共享资源实体的模式及release_resource_owner回收模式
    LibraryComponent由空标记改为提供自身ID的方法，新增ResourcesComponent
    原有ToolResource、PromptResource、SandboxResource不再作为独立图书馆组件挂载
    AgentImage同样迁为ResourceSet<AgentImage>中的资源条目，删除其LibraryComponent身份
    移除依赖计数和资源GC计划，依靠实体拥有组件的生命周期释放资源条目
    Workspace等领域文档与本契约同步；基础设施不保留ResourceRef、独立镜像Entity或GC作为兼容目标
    本文不落实其他插件的字段迁移、事件改造、磁盘加载接口或执行快照实现
```
