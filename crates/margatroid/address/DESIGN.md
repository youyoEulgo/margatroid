# 伪代码格式

```text
板块：一级标题，按职责列出实际文件，不创建空模块。
类型、函数：二级标题，区分公开与私有。
TypeName：中文名称，可见性类型--用途
    field: Type--字段说明
    method(...) -> ReturnType
        行为：直接描述操作及错误
函数：写明签名和直接行为。
属性：不写Rust Attribute。
持有关系：文末使用树形图。
```

# 板块格式

```text
# lib        address/src/lib.rs          公开入口与World节点寻址、查重扩展
# components address/src/components.rs   实体节点Address组件
# handler    address/src/handler.rs      文件资源路径定位
# types pub  address/src/types.rs        文件资源地址解析值
# error      address/src/error.rs        解析、寻址与文件定位错误
```

# lib

```text
crate address：core
    下层依赖：mecs::core_plugin中的Component、Entity、World
    通用依赖：标准库
    禁止依赖：其他业务core、任意业务plugin
    调用方向：plugin → address → mecs
    职责：提供节点地址组件、World查询、文件资源地址解析及文件定位。
        文件定位只使用标准库，不因涉及IO而划入plugin。
        本设计不增加业务plugin、System或事件；不负责跨领域装配和生命周期编排。
```

## 类型

公开：
```text
WorldAddressExt：实体节点地址查询扩展，公开trait
    entity_by_address(&self, address: &str) -> Result<Entity, AddressError>
        行为：通过Address::new校验完整节点路径，扫描Address组件并精确匹配；零个匹配返回EntityMissing，一个返回完整Entity，多个返回DuplicateAddress。
    address_exists(&self, address: &str) -> Result<bool, AddressError>
        行为：通过Address::new校验完整节点路径，扫描Address组件；存在精确匹配返回true，否则false。
    impl WorldAddressExt for World
        行为：直接查询Address组件，不使用注册表或地址缓存；不接受文件资源地址，不按名称、路径前缀或父节点隐式查找。
```

# components

## 类型

公开：
```text
Address：实体节点地址，公开组件
    value: String--私有，完整绝对节点路径，如/workspace/agent/subagent
    new(value: &str) -> Result<Self, AddressError>
        行为：只接受以/开头的节点路径；/表示根节点，其余路径由一个或多个非空节点段组成。
            拒绝相对路径、空段、尾部/、节点段.或..、反斜杠、冒号及控制字符；非法输入返回InvalidNodePath。
            校验后原样保存，按大小写精确区分；不补齐前导/，不折叠路径，不解析文件系统或符号链接。
            节点路径不带kind/scope/tag，不调用ParsedAddress::parse；type:scope/name:tag文件资源地址不得挂载为实体Address。
            只构造地址值，不创建实体或父节点，不建立父子所有权，不根据节点路径派生磁盘目录。
    as_str(&self) -> &str
        行为：返回完整节点路径。
    impl Component for Address
    impl Clone + Debug + PartialEq + Eq for Address
```

# handler

## 函数

公开：
```text
resolve_file(root: &Path, address: &str) -> Result<PathBuf, AddressError>
    行为：
        解析type:scope/name:tag，必须显式提供非空tag，不补齐默认值。
        按资源类型的磁盘布局解析真实入口文件，类型对应根目录由调用方提供。
        规范化root与候选路径，要求候选仍位于root内且为真实普通文件。
        返回规范绝对路径；格式错误、越界、不存在或非文件时返回错误。
```

# types pub

## 类型

公开：
```text
ParsedAddress：解析后的文件资源地址，公开值类型
    kind: String--类型段
    scope: String--作用域
    name: String--名称
    tag: String--标签
    parse(value: &str) -> Result<Self, AddressError>
        行为：接受type:scope/name:tag；类型为小写ASCII字母、数字、下划线或短横线；其余段非空且不含路径分隔符、控制字符或冒号；拒绝点及双点；必须显式提供非空tag，不补齐默认值。
    impl Display for ParsedAddress
        行为：输出type:scope/name:tag。
```

# error

## 类型

公开：
```text
AddressError：地址错误，公开枚举
    InvalidFormat
    InvalidPart
    InvalidNodePath
    EntityMissing
    DuplicateAddress
    PathOutsideRoot
    FileMissing
    NotFile
    Io(std::io::Error)
    impl Display for AddressError
        行为：InvalidNodePath说明实体要求绝对节点路径；InvalidFormat及InvalidPart保留文件资源格式和段校验含义，其余错误输出分类及必要路径信息。
    impl std::error::Error for AddressError
```

# 持有关系

```text
业务plugin
└── address（core）
    └── mecs::core_plugin

World
└── Entity
    ├── Address
    │   └── 完整绝对节点路径
    └── 领域组件

调用方
├── ParsedAddress（文件资源解析期间的临时值）
└── resolve_file返回的PathBuf

各领域集合组件
└── BTreeMap<String, PathBuf>
    ├── key：alias
    └── value：实际文件路径
```
