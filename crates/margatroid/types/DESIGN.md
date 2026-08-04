# MargatroidTypes

## 类型

公开：
```text
ResourceNameError：资源名称错误，公开枚举--描述scope/name逻辑名称的格式错误
    Empty
    InvalidScope
    InvalidName
    InvalidCharacter
    impl fmt::Display for ResourceNameError
        Display：公开trait实现，输出不包含原始输入的稳定错误描述
    impl std::error::Error for ResourceNameError
        Error：公开trait实现

ResourceName：资源逻辑名称，公开结构体--跨资源Loader共享scope/name标识
    scope: String--资源作用域，私有
    name: String--作用域内名称，私有
    new(value: impl Into<String>) -> Result<Self, ResourceNameError>
        构造名称：公开关联函数，解析并验证scope/name文本
        行为：
            value为空时返回Empty
            value必须恰好包含scope和name两个非空路径段
            scope和name不能是.或..
            scope和name不能包含控制字符或反斜杠
            成功时分别保存scope和name
    scope(&self) -> &str
        取得作用域：公开方法，返回scope
    name(&self) -> &str
        取得名称：公开方法，返回name
    impl fmt::Display for ResourceName
        Display：公开trait实现，输出scope/name

AgentImageReferenceError：AgentImage引用错误，公开枚举--描述scope/name:tag格式错误
    InvalidName
    InvalidTag
    impl fmt::Display for AgentImageReferenceError
        Display：公开trait实现，输出不包含原始输入的稳定错误描述
    impl std::error::Error for AgentImageReferenceError
        Error：公开trait实现

AgentImageReference：AgentImage引用，公开结构体--跨Loader和Workspace共享的规范化scope/name:tag标识
    resource: ResourceName--镜像scope/name，私有
    tag: String--镜像版本标签，私有
    new(value: impl Into<String>) -> Result<Self, AgentImageReferenceError>
        构造引用：公开关联函数，解析并验证scope/name:tag文本
        行为：
            tag省略时使用latest
            scope/name必须满足ResourceName规则
            tag长度为1到128 UTF-8字节
            tag只允许ASCII字母、数字、下划线、点和连字符
            tag首字符不能是点或连字符
            成功时保存ResourceName和规范化tag
    resource(&self) -> &ResourceName
        取得资源名：公开方法，返回scope/name
    scope(&self) -> &str
        取得作用域：公开方法，返回resource.scope
    name(&self) -> &str
        取得名称：公开方法，返回resource.name
    tag(&self) -> &str
        取得标签：公开方法，返回tag
    impl fmt::Display for AgentImageReference
        Display：公开trait实现，始终输出scope/name:tag
```

## 函数

私有：
```text
validate_part(part: &str) -> Result<(), ResourceNameError>
    验证名称段：私有函数，拒绝空值、.、..、控制字符和反斜杠

validate_tag(tag: &str) -> Result<(), AgentImageReferenceError>
    验证标签：私有函数，执行AgentImageReference的长度、字符和首字符规则

is_tag_character(character: char) -> bool
    检查标签字符：私有函数，只接受ASCII字母、数字、下划线、点和连字符
```

## 逻辑

```text
构造：
    ResourceName::new(value)
        -> 按/拆分value
        -> 必须恰好得到scope和name
        -> 分别调用validate_part
        -> 保存两个名称段

作为资源键：
    AgentImageLoaderPlugin发现scope/name目录
        -> 构造ResourceName
        -> SkillLoaderPlugin和WorkflowLoaderPlugin共享同一值类型
        -> 各Loader按scope与name拼接自己的受控资源根

构造AgentImage引用：
    AgentImageReference::new(value)
        -> 按第一个冒号拆分scope/name与可选tag
        -> 没有tag时使用latest
        -> ResourceName::new(scope/name)
        -> validate_tag(tag)
        -> 保存resource与tag
```

## 持有关系

```text
ResourceName
├── scope: String
└── name: String

AgentImageReference
├── resource: ResourceName
└── tag: String
```
