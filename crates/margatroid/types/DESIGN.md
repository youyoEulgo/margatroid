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
```

## 函数

私有：
```text
validate_part(part: &str) -> Result<(), ResourceNameError>
    验证名称段：私有函数，拒绝空值、.、..、控制字符和反斜杠
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
```

## 持有关系

```text
ResourceName
├── scope: String
└── name: String
```
