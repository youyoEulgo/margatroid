# lib

## 类型

公开：
```text
ResourceIdPlugin：统一资源身份插件，公开单元结构体--提供ResourceId组件和World按ID查询能力
    impl Default for ResourceIdPlugin
    impl Plugin for ResourceIdPlugin
        build(self, app: &mut App)
            安装插件：要求CorePlugin已安装
            行为：
                重复安装时panic
                插入ResourceIdPluginInstalled
                不创建索引Resource，不挂载System

ResourceIdPluginInstalled：ResourceIdPlugin安装标记，公开Resource
    impl Resource for ResourceIdPluginInstalled
```

## 逻辑

```text
插件职责：
    定义所有可寻址Entity统一使用的ResourceId组件
    为World提供按规范化ResourceId查询当前Entity的方法
    查询直接以World当前组件为准，不维护第二份ResourceId到Entity索引

插件边界：
    ResourceIdPlugin不创建或销毁Entity
    ResourceIdPlugin不判断资源的领域类型是否允许挂载到某个Entity
    ResourceIdPlugin不解析Workspace、Agent、Image、Skill、Tool或MCL语义
    各领域Plugin负责创建Entity并挂载ResourceId，查询统一调用WorldResourceIdExt
```

# events

## 类型

```text
ResourceIdPlugin不定义事件
```

# types

## 类型

公开：
```text
ResourceId：统一资源ID，公开Component--所有可寻址Entity共享的稳定身份
    resource_type: String--资源类型，私有
    scope: String--资源命名空间，私有
    name: String--命名空间内名称，私有
    tag: String--版本或实例标签，私有；省略时规范化为latest
    parse(value: impl AsRef<str>) -> Result<Self, ResourceIdError>
        解析ID：公开关联函数，解析type:scope/name[:tag]并补齐latest
    new(resource_type: impl Into<String>, scope: impl Into<String>, name: impl Into<String>, tag: Option<impl Into<String>>) -> Result<Self, ResourceIdError>
        构造ID：公开关联函数，验证字段并在tag为空时补齐latest
    resource_type(&self) -> &str
    scope(&self) -> &str
    name(&self) -> &str
    tag(&self) -> &str
    impl fmt::Display for ResourceId
        Display：始终输出type:scope/name:tag
    impl FromStr for ResourceId
        FromStr：行为与parse一致
    impl Clone + Ord + Eq + Hash for ResourceId
    impl Serialize + Deserialize for ResourceId
        序列化：使用规范化完整ID字符串
    impl Component for ResourceId

WorldResourceIdExt：World统一资源寻址扩展，公开trait
    entity_by_resource_id(&self, id: &ResourceId) -> Result<Entity, ResourceIdLookupError>
        按ID查询Entity：要求World已经安装ResourceIdPlugin；返回当前唯一匹配Entity
    impl WorldResourceIdExt for World
```

## 逻辑

```text
格式：
    ResourceId规范格式为type:scope/name:tag
    省略tag时使用latest
    四个字段共同参与比较和哈希

身份：
    ResourceId是跨Plugin查询资源的稳定地址，不等同于Entity句柄
    Agent、Workspace、AgentImage及其他可寻址运行时对象都挂载ResourceId组件
    DTO、配置、日志、Memory和前端统一使用规范化ResourceId
```

# system

## 函数

```text
ResourceIdPlugin没有System
```

# error

## 类型

公开：
```text
ResourceIdError：ResourceId解析和验证错误，公开枚举
    Empty
    InvalidType
    InvalidScope
    InvalidName
    InvalidTag
    InvalidFormat
    impl fmt::Display for ResourceIdError
    impl std::error::Error for ResourceIdError

ResourceIdLookupError：World资源寻址错误，公开枚举
    PluginMissing--World未安装ResourceIdPlugin
    Missing { id: ResourceId }--没有Entity挂载目标ID
    Duplicate { id: ResourceId, entities: Vec<Entity> }--多个Entity挂载同一ID
    impl fmt::Display for ResourceIdLookupError
        Display：输出稳定错误类型和规范化ResourceId，不输出组件内容
    impl std::error::Error for ResourceIdLookupError
```

# handler

## 函数

公开：
```text
entity_by_resource_id(world: &World, id: &ResourceId) -> Result<Entity, ResourceIdLookupError>
    按ID查询：公开函数
    行为：
        检查ResourceIdPluginInstalled存在
        遍历所有挂载ResourceId组件的存活Entity
        筛选ResourceId与id完全相等的Entity
        无匹配返回Missing
        唯一匹配返回该Entity
        多个匹配按Entity稳定顺序排列后返回Duplicate
```

## 不变量

```text
查询只返回当前仍存活且当前挂载目标ResourceId的Entity
查询不得依赖Workspace、Agent或其他领域索引
查询不得在重复ID时静默选择第一个Entity
Entity创建和销毁不需要同步ResourceIdPlugin缓存，因为不存在第二份索引
```
