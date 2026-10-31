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
# lib        address/src/lib.rs          公开入口与World寻址、查重扩展
# components address/src/components.rs   Address组件
# handler    address/src/handler.rs      文件路径定位
# types pub  address/src/types.rs        地址解析值
# error      address/src/error.rs        解析、寻址与文件定位错误
```

# lib

## 类型

公开：
```text
WorldAddressExt：实体地址查询扩展，公开trait
    entity_by_address(&self, address: &str) -> Result<Entity, AddressError>
        行为：校验地址，扫描Address组件；零个匹配返回EntityMissing，一个返回完整Entity，多个返回DuplicateAddress。
    address_exists(&self, address: &str) -> Result<bool, AddressError>
        行为：校验地址，扫描Address组件；存在匹配返回true，否则false。
    impl WorldAddressExt for World
        行为：直接查询Address组件，不使用注册表或地址缓存。
```

# components

## 类型

公开：
```text
Address：实体对外地址，公开组件
    value: String--私有，规范地址字符串
    new(value: &str) -> Result<Self, AddressError>
        行为：解析并保存规范地址。
    as_str(&self) -> &str
        行为：返回地址字符串。
    impl Component for Address
    impl Clone + Debug + PartialEq + Eq for Address
```

# handler

## 函数

公开：
```text
resolve_file(root: &Path, address: &str) -> Result<PathBuf, AddressError>
    行为：
        解析type:scope/name[:tag]，缺省tag为latest。
        按资源类型的磁盘布局解析真实入口文件，类型对应根目录由调用方提供。
        规范化root与候选路径，要求候选仍位于root内且为真实普通文件。
        返回规范绝对路径；格式错误、越界、不存在或非文件时返回错误。
```

# types pub

## 类型

公开：
```text
ParsedAddress：解析后的地址，公开值类型
    kind: String--类型段
    scope: String--作用域
    name: String--名称
    tag: String--标签
    parse(value: &str) -> Result<Self, AddressError>
        行为：接受type:scope/name[:tag]；类型为小写ASCII字母、数字、下划线或短横线；其余段非空且不含路径分隔符、控制字符或冒号；拒绝点及双点；缺省tag为latest。
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
    EntityMissing
    DuplicateAddress
    PathOutsideRoot
    FileMissing
    NotFile
    Io(std::io::Error)
    impl Display for AddressError
        行为：输出错误分类及必要路径信息。
    impl std::error::Error for AddressError
```

# 持有关系

```text
World
└── Entity
    ├── Address
    │   └── 规范地址字符串
    └── 领域组件

调用方
├── ParsedAddress（解析期间的临时值）
└── resolve_file返回的PathBuf

各领域集合组件
└── BTreeMap<String, PathBuf>
    ├── key：alias
    └── value：实际文件路径
```
