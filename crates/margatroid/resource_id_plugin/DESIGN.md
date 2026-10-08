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
# lib        src/lib.rs        Plugin 与扩展 trait
# system     src/system.rs     System 函数（当前无）
# handler    src/handler.rs    处理函数（当前无）
# events     src/events.rs     事件类型（当前无）
# types      src/types.rs      ResourceId 组件与解析
# error      src/error.rs      Error 类型
```

# lib

## 类型

公开：
```text
ResourceIdPlugin：统一资源身份插件，公开单元结构体--提供 ResourceId 组件和 World 按 ID 查询能力
    impl Plugin for ResourceIdPlugin
        build(self, app: &mut App)
            安装插件：重复安装时 panic；插入 ResourceIdPluginInstalled；不创建索引 Resource，不挂载 System

ResourceIdPluginInstalled：ResourceIdPlugin 安装标记，公开单元 Resource
    impl Resource for ResourceIdPluginInstalled
```

## 扩展 trait

公开：
```text
WorldResourceIdExt：World 资源地址扩展，公开 trait
    entity_by_resource_id(&self, id: &ResourceRef) -> Result<Entity, ResourceIdLookupError>
        按资源地址查询唯一 Entity：要求插件已安装；缺失返回 Missing，多条返回 Duplicate
    attach_resource_id(&self, entity: Entity, id: ResourceRef) -> Result<(), ResourceIdAttachError>
        挂载资源地址：不允许另一个 Entity 占用同一地址
        行为：同一 Entity 重复挂载同一地址是幂等的
        行为：挂载失败时 Entity 保持原有身份不变
        行为：Entity 不存活时返回 MissingEntity
        说明：底层 ECS 直接插入组件可以绕过受检入口，因此查询仍然必须把
            重复地址诊断为 Duplicate，而不是当作缺失
    impl WorldResourceIdExt for World
        实现：以 World 当前组件为准，不维护第二份索引
```

空板块：
    某个板块在本 crate 中没有内容时，对应的源文件保持为空并仍然保留模块声明
    空文件不是遗漏，而是"该板块在此 crate 中确实没有定义"的表示
    保留文件与模块声明是为了让六个板块的位置固定，新增内容时不必再改结构

# system

当前无 System。

# handler

当前无处理函数。

# events

当前无事件类型。

# types

## 类型

公开：
```text
ResourceRef：资源地址值，公开类型--资源身份本身，可自由用作字段、集合键与序列化载荷
    resource_type: String--资源类型，私有
    scope: String--作用域，私有
    name: String--资源名，私有
    tag: String--标签，私有
    parse(value: impl AsRef<str>) -> Result<Self, ResourceIdError>
        解析资源地址：公开关联函数，调用 FromStr
    new(resource_type, scope, name, tag: Option) -> Result<Self, ResourceIdError>
        构造资源地址：公开关联函数，tag 省略时为 latest
    resource_type(&self) -> &str
        读取类型：公开方法
    scope(&self) -> &str
        读取作用域：公开方法
    name(&self) -> &str
        读取名称：公开方法
    tag(&self) -> &str
        读取标签：公开方法
    impl FromStr for ResourceRef
        解析：格式为 type:scope/name[:tag]，tag 省略时为 latest
    impl fmt::Display for ResourceRef
        输出：type:scope/name:tag
    impl Serialize for ResourceRef
        序列化：写为字符串
    impl Deserialize for ResourceRef
        反序列化：从字符串解析
    impl Ord + PartialOrd + Hash
        可排序、可哈希：因此能作为 BTreeMap 与 HashMap 的键

ResourceId：资源地址组件，公开类型--把 ResourceRef 挂载到 Entity 上的包装
    impl Component for ResourceId
    说明：ResourceRef 不实现 Component，只有 ResourceId 可以挂载

身份与挂载分离：
    同一个值曾经既是地址又是组件，导致"引用"与"实例"难以区分
    现在 ResourceRef 只是值，ResourceId 只是挂载载体，二者职责不再重叠
    资源库据此把"按地址查找宿主 Entity"与"把地址挂到 Entity"表达成两个不同动作
    引用不实现 Component 这一点由类型系统保证：没有 Component 实现就无法被挂载

私有：
```text
validate_type(value: &str) -> Result<(), ResourceIdError>
    校验类型：小写字母、数字、_ 或 -
validate_part(value: &str, error: ResourceIdError) -> Result<(), ResourceIdError>
    校验作用域或名称：非空、非 . 或 ..、不含控制字符、/、\、:
validate_tag(value: &str) -> Result<(), ResourceIdError>
    校验标签：首字符为字母数字，后续允许字母数字、_、-、.
```

# error

## 类型

公开：
```text
ResourceIdError：ResourceId 错误，公开枚举
    Empty
    InvalidType
    InvalidScope
    InvalidName
    InvalidTag
    InvalidFormat
    impl Clone + Debug + PartialEq + Eq for ResourceIdError
    impl fmt::Display for ResourceIdError
    impl std::error::Error for ResourceIdError

ResourceIdLookupError：ResourceId 查询错误，公开枚举
    PluginMissing
    Missing { id: ResourceId }
    Duplicate { id: ResourceId, entities: Vec<Entity> }
    impl Clone + Debug + PartialEq + Eq for ResourceIdLookupError
    impl fmt::Display for ResourceIdLookupError
    impl std::error::Error for ResourceIdLookupError
```

# 逻辑

```text
插件职责：
    定义所有可寻址 Entity 统一使用的 ResourceId 组件
    为 World 提供按规范化 ResourceId 查询当前 Entity 的方法
    查询直接以 World 当前组件为准，不维护第二份索引

插件边界：
    ResourceIdPlugin 不创建或销毁 Entity
    ResourceIdPlugin 不判断资源的领域类型是否允许挂载到某个 Entity
    各领域 Plugin 负责创建 Entity 并挂载 ResourceId
```

# 持有关系

```text
App
└── World
    ├── ResourceIdPluginInstalled
    └── 各 Entity
        └── ResourceId 组件
