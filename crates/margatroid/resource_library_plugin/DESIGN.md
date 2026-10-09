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

每个 crate 的 DESIGN.md 在伪代码格式之后使用六个一级标题，按 `lib`、`system`、`handler`、`events`、`types`、`error` 顺序组织。每个标题对应 `src/` 下的同名 Rust 文件：

```text
# lib        src/lib.rs        资源定义组件、库根 Resource 与 Plugin
# system     src/system.rs     System 函数（当前无）
# handler    src/handler.rs    资源安装与回收
# access     src/access.rs     资源的只读查询与按需读盘
# events     src/events.rs     事件类型（当前无）
# types      src/types.rs      ResourceDefinition 与 ResourceOwners
# error      src/error.rs      Error 类型

access 是六个标准板块之外的一个板块：handler 负责改变资源，access 只读取资源，
    两者都是本 crate 对外的主要入口，但职责相反，混在一起会让「谁改、谁读」变得模糊
```

# lib

## 类型

公开：
```text
ToolResource：工具资源组件，公开类型，图书馆组件--记录工具描述、输入 schema 与脚本路径
    description: String--工具描述，同时作为 Lua 工具元数据
    input_schema: serde_json::Value--输入 JSON schema
    script_path: PathBuf--main.lua 的绝对路径，导入时确定
    impl Component for ToolResource
    impl LibraryComponent for ToolResource

SkillResource：技能资源组件，公开类型，图书馆组件--记录技能描述与正文路径
    description: String--技能描述
    content_path: PathBuf--SKILL.md 的绝对路径，导入时确定
    impl Component for SkillResource
    impl LibraryComponent for SkillResource

PromptResource：提示资源组件，公开类型，图书馆组件--记录角色与正文
    role: String--system、user 或 assistant
    content: Arc<str>--正文，导入时读入内存
    impl Component for PromptResource
    impl LibraryComponent for PromptResource

SandboxResource：沙箱资源组件，公开类型，图书馆组件--记录策略
    policy: Arc<str>--策略 JSON 文本
    impl Component for SandboxResource
    impl LibraryComponent for SandboxResource

HookResource：钩子资源组件，公开类型，图书馆组件--记录描述与输入 schema
    description: String--钩子描述
    input_schema: serde_json::Value--输入 JSON schema
    impl Component for HookResource
    impl LibraryComponent for HookResource

ResourceLibraryRoot：资源库根目录，公开 Resource--全局资源库的绝对根路径
    impl Resource for ResourceLibraryRoot

ResourceLibraryPlugin：资源库插件，公开类型--持有根路径
    root: PathBuf--资源库根路径
    new(root: impl Into<PathBuf>) -> Self
        新建：构造插件并保存根路径
    impl Plugin for ResourceLibraryPlugin
        build(self, app: &mut App)
            安装插件：断言 ResourceIdPlugin 已安装，且 ResourceLibraryRoot 尚未存在；插入 ResourceLibraryRoot；不注册 System
```

## 边界的由来

镜像不再存资源，资源全局唯一。任何 `ResourceRef` 只在整个进程中存在一份内容，因此一个引用只对应一个宿主 Entity；导入是纯增量，同一资源被多个 Agent 导入时复用同一 Entity。

图书馆组件与 Entity 一一对应。资源 Entity 只挂载这里的五种组件之一，外加 `ResourceId` 与 `ResourceOwners`；挂载哪个组件决定资源类型，也决定 `ResourceDefinition` 的类型匹配是否成立。

`ResourceOwners` 记录持有者，而不是计数。持有者用完整 `Entity`（index + generation）表示，因此 Entity 索引被回收再分配后，新 Entity 不会继承旧持有者的所有权。

# handler

公开：
```text
install_resource(world: &mut World, reference: ResourceRef, owner: Entity, definition: ResourceDefinition) -> Result<Entity, ResourceLibraryError>
    安装资源：把定义挂到 reference 对应的宿主 Entity，并登记 owner 为持有者
    行为：owner 不存活则返回 OwnerMissing
    行为：reference 的类型与 definition 的类型不一致则返回 TypeMismatch
    行为：按 reference 查询宿主 Entity
    行为：命中时校验其已挂载的组件与 definition 同类，且存在 ResourceOwners；否则返回 InvalidHost
    行为：未命中且为 Missing 时生成新 Entity，挂载 ResourceId，插入空的 ResourceOwners
    行为：查找错误为其它情形时直接返回该错误
    行为：用 definition 覆盖宿主 Entity 上的资源组件，因此重复导入以最新内容为准
    行为：把 owner 加入 ResourceOwners；同一 owner 重复加入保持幂等
    行为：返回宿主 Entity

release_resource_owner(world: &mut World, owner: Entity)
    释放持有者：从所有资源中移除 owner，并回收不再被持有的资源
    行为：遍历所有挂载 ResourceOwners 的 Entity
    行为：移除 owner 的持有记录
    行为：持有者集合变空时 despawn 该 Entity
    行为：不触碰磁盘上的资源包，磁盘内容不属于运行时生命周期
```

私有：
```text
matches_definition(world: &World, entity: Entity, definition: &ResourceDefinition) -> bool
    定义是否同类：判断 Entity 已挂载的资源组件是否与定义同类型
    行为：按定义变体查询对应组件是否存在
```

## 回收时机的约束

资源回收由请求停止的一方驱动，而不是由导入的一方驱动。Agent 在存活期间纯增量：导入只增加引用，从不主动释放。

只有在 Agent 的在途工作真正停止后才允许释放其持有，否则运行中的调用会失去自己的资源。停止收敛的判据由 `workspace_plugin` 定义：VM 已结束（vm_id 与 request_id 均为 None）、生命周期为 Stopped 或 Failed、且 tools.pending 与 inference.pending 均为空。

生命周期标签本身不足以证明执行已结束：一个进入 Failed 的 VM 仍可能持有正在运行的执行体，因此取消判定必须以在途请求为准。

# access

access 是本 crate 的读取面，也是全局资源库作为唯一数据来源的落点。

## 类型

公开：
```text
ExecutableDefinition：可执行资源对模型可见的定义，公开结构体
    description: String--描述
    input_schema: serde_json::Value--输入 schema
```

## 函数

公开：
```text
find_resource(world: &World, reference: &ResourceRef) -> Result<Entity, ResourceLibraryError>
    查找资源实体：返回该地址当前对应的 Entity
    行为：查询失败时原样返回 ResourceIdLookupError

tool_resource(world: &World, reference: &ResourceRef) -> Result<ToolResource, ResourceLibraryError>
    读取工具资源：返回其组件的克隆
    行为：地址不存在返回 Lookup；实体不是工具资源返回 WrongType
    说明：返回克隆而不是引用，调用方不必持有 World 的借用

skill_resource / prompt_resource / sandbox_resource / hook_resource
    读取对应资源：与 tool_resource 同形，各自校验资源类型

tool_definition(world: &World, reference: &ResourceRef) -> Result<ExecutableDefinition, ResourceLibraryError>
    读取可执行定义：按资源类型返回对模型可见的描述与 schema
    行为：tool 与 hook 返回各自组件中的描述与 schema
    行为：skill 的 schema 固定为空对象 schema，因为技能不接受结构化参数
    行为：其它类型返回 WrongType

read_tool_script(world: &World, reference: &ResourceRef) -> Result<String, ResourceLibraryError>
    读取工具脚本：按 ToolResource.script_path 读盘
read_skill_content(world: &World, reference: &ResourceRef) -> Result<String, ResourceLibraryError>
    读取技能正文：按 SkillResource.content_path 读盘
    行为：读盘失败返回 ContentUnreadable，并带上资源地址与底层原因
```

## 唯一数据来源

图书馆是资源的唯一数据来源，其它地方只保存引用：

```text
资源内容（描述、schema、策略、正文、路径）一律只存在于资源实体的组件上
Agent 一侧只保存 ResourceRef，外加真正属于该 Agent 的私有属性
    模型的可见名（别名后的名字）与执行器标签属于 Agent 视角，不是资源自身的属性
任何调用方都不自行拼接资源库路径，需要内容时通过 access 读取
```

内存与磁盘的分工：

```text
描述、schema、策略、提示正文在 import 时读入内存并写入组件
    这些东西小，且每次调用都要用，读盘没有收益
工具脚本与技能正文只保存路径，由 access 按需读取
    它们可能很大，且一次调用只读一次
不落盘的内容不保留原始文件文本：
    tool.toml 的 schema_version 与 name 是导入期的校验依据，校验完成后不再保留
        资源的身份已由 ResourceId 承载，再存一份 name 就是重复
    校验因此只在 import 时发生一次
```

调用方不拼接路径：
    需要脚本或正文时调用 read_tool_script / read_skill_content
    库根的拼接与读盘失败的错误包装都收在 access 内
    调用方拿到的要么是内容，要么是一个带上资源地址的错误

描述文本只是数据：
    描述到达执行体时是 String，不是待解析的文档
    因此描述里出现 "="、":" 之类的字符不会影响任何一次调用
    测试用一句会被 TOML 拒绝的描述来钉住这一点

这样安排的直接后果是执行路径不再解析 TOML：

```text
执行工具时读的是组件里已经校验过的描述与 schema
    而不是把描述文本当成 TOML 文档再解析一遍
类型上也不可能再犯这个错：LuaToolPackage 装的是 String 与 serde_json::Value，
    而不是两个语义不明的 String
```

# types

公开：
```text
ResourceDefinition：资源定义，公开枚举--导入时携带的资源内容，五种变体与五个资源组件一一对应
    Tool(ToolResource)
    Skill(SkillResource)
    Prompt(PromptResource)
    Sandbox(SandboxResource)
    Hook(HookResource)
    resource_type(self) -> &'static str
        资源类型名：返回该定义对应的资源类型
        行为：按变体返回 tool、skill、prompt、sandbox 或 hook

ResourceOwners：资源持有者集合，公开类型--记录当前持有该资源的 Agent Entity
    agents: HashSet<Entity>--持有者集合，元素为完整 Entity，私有字段
    impl Component for ResourceOwners
    contains(&self, agent: Entity) -> bool
        是否持有：判断该 Agent 是否为持有者
    len(&self) -> usize
        持有者数量：返回集合长度
    is_empty(&self) -> bool
        是否无人持有：判断集合是否为空
    acquire(&mut self, agent: Entity)
        取得持有：加入持有者集合，已存在时不重复；crate 内可见
    release(&mut self, agent: Entity)
        释放持有：从持有者集合移除；crate 内可见
```

ResourceOwners 不是图书馆组件：它描述资源与 Agent 之间的关系，而不是资源自身的领域身份。
