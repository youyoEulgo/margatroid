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
WorldResourceIdExt：World 资源 ID 查询扩展，公开 trait
    entity_by_resource_id(&self, id: &ResourceId) -> Result<Entity, ResourceIdLookupError>
        按资源 ID 查询唯一 Entity：要求插件已安装；缺失返回 Missing，多条返回 Duplicate
    impl WorldResourceIdExt for World
        实现：以 World 当前组件为准，不维护第二份索引
```

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
ResourceId：统一资源 ID 组件，公开类型--所有可寻址 Entity 统一使用的身份组件
    resource_type: String--资源类型，私有
    scope: String--作用域，私有
    name: String--资源名，私有
    tag: String--标签，私有
    parse(value: impl AsRef<str>) -> Result<Self, ResourceIdError>
        解析资源 ID：公开关联函数，调用 FromStr
    new(resource_type, scope, name, tag: Option) -> Result<Self, ResourceIdError>
        构造资源 ID：公开关联函数，tag 省略时为 latest
    resource_type(&self) -> &str
        读取类型：公开方法
    scope(&self) -> &str
        读取作用域：公开方法
    name(&self) -> &str
        读取名称：公开方法
    tag(&self) -> &str
        读取标签：公开方法
    impl FromStr for ResourceId
        解析：格式为 type:scope/name[:tag]，tag 省略时为 latest
    impl fmt::Display for ResourceId
        输出：type:scope/name:tag
    impl Serialize for ResourceId
        序列化：写为字符串
    impl Deserialize for ResourceId
        反序列化：从字符串解析
    impl Component for ResourceId

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
