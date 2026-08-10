# Compose

## 类型

公开：
```text
ComposeErrorKind：Workspace文件编译错误分类，公开枚举--描述读取、解析和静态校验失败
    FileRead
    FileDecode
    InvalidDefinition
    InvalidPath
    InvalidName
    InvalidImageReference
    InvalidResourceReference
    DuplicateAgent
    MissingManager
    impl Clone + Copy + PartialEq + Eq for ComposeErrorKind
        值语义：公开trait实现

ComposeError：Workspace文件编译错误，公开结构体--保存稳定分类和有界描述
    kind: ComposeErrorKind--错误分类，私有
    message: String--不包含完整配置正文的稳定描述，私有
    kind(&self) -> ComposeErrorKind
        取得分类：公开方法，返回错误分类
    message(&self) -> &str
        取得描述：公开方法，返回有界错误描述
    new(kind: ComposeErrorKind, message: impl Into<String>) -> Self
        构造错误：私有关联函数，保存分类和有界描述
    impl Clone + PartialEq + Eq for ComposeError
        值语义：公开trait实现
    impl fmt::Display for ComposeError
        Display：公开trait实现，输出分类和描述
    impl std::error::Error for ComposeError
        Error：公开trait实现
```

私有：
```text
WorkspaceFile：YAML顶层文档，私有结构体--保存反序列化后的原始静态字段
    name: String--Workspace逻辑名称
    project_root: Option<PathBuf>--相对workspace文件目录的项目根
    manager: Option<String>--默认入口Agent名称
    agents: AgentDocuments--Agent映射或列表

AgentDocuments：Agent文档集合，私有结构体--保留配置出现顺序
    0: Vec<RawAgent>--原始Agent配置
    impl Deserialize for AgentDocuments
        Deserialize：公开trait实现，接受名称映射或带name字段的列表
        deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            反序列化：使用AgentDocumentsVisitor接受映射或序列

AgentDocumentsVisitor：Agent集合访问器，deserialize内私有局部单元结构体
    impl Visitor for AgentDocumentsVisitor
        Value = AgentDocuments--反序列化结果
        expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result
            描述输入：要求Agent定义映射或序列
        visit_map<A>(self, map: A) -> Result<AgentDocuments, A::Error>
            读取映射：把键写入RawAgent.name；内部name存在且与键不一致时失败
        visit_seq<A>(self, sequence: A) -> Result<AgentDocuments, A::Error>
            读取序列：按原顺序收集RawAgent，name必填校验留给build_definition

RawAgent：Agent原始文档，私有结构体--等待编译为WorkspaceAgentDefinition
    name: Option<String>--列表中的Agent名称；映射形式由键赋值
    image: String--AgentImage引用文本
    resources: Vec<RawResource>--额外可见资源
    disable_resources: Vec<RawResource>--禁用资源
    memory_path: Option<PathBuf>--Memory SQLite路径

RawResource：资源原始引用，私有枚举--接受结构化引用或provider:scope/name简写
    Structured(RawStructuredResource)
    Shorthand(String)

RawStructuredResource：结构化资源引用，私有结构体--保存provider和逻辑名称
    provider: String--工具定义Plugin ID
    name: String--scope/name逻辑名称
```

## 函数

公开：
```text
compile(path: impl AsRef<Path>) -> Result<WorkspaceDefinition, ComposeError>
    编译Workspace文件：公开函数，读取并编译给定YAML文件
    行为：将文件路径规范化后读取UTF-8 YAML，并以其父目录作为相对路径基准，返回静态WorkspaceDefinition

compile_str(source: &str, workspace_file: impl AsRef<Path>) -> Result<WorkspaceDefinition, ComposeError>
    编译Workspace文本：公开函数，使用给定workspace文件路径的父目录解析相对路径
    行为：反序列化YAML，校验所有逻辑名称、AgentImage引用、ResourceRef、Agent唯一性和manager，返回静态定义
```

私有：
```text
build_definition(raw: WorkspaceFile, base: &Path) -> Result<WorkspaceDefinition, ComposeError>
    构造Workspace定义：私有函数，将原始文档转换为margatroid_types值类型
    行为：解析项目根、按配置顺序构造所有Agent，默认manager为第一个Agent，检查唯一性和manager存在

parse_resource(raw: RawResource) -> Result<ResourceRef, ComposeError>
    解析资源：私有函数，构造ResourceName和ResourceRef；结构化引用直接读取，简写按provider:scope/name拆分

validate_logical_name(value: &str, kind: &str) -> Result<(), ComposeError>
    校验逻辑名称：私有函数，执行WorkspacePlugin相同的单路径段、长度和控制字符约束

absolute_path(path: &Path) -> Result<PathBuf, ComposeError>
    取得绝对路径：私有函数，将相对路径按当前工作目录转换并执行词法规范化

resolve_path(base: &Path, path: PathBuf) -> Result<PathBuf, ComposeError>
    解析相对路径：私有函数，将相对path按base转换为绝对路径并拒绝parent traversal

normalize_path(path: PathBuf) -> Result<PathBuf, ComposeError>
    规范化路径：私有函数，移除当前目录组件并拒绝ParentDir，返回绝对路径
```

## 逻辑

```text
compile(path)
    -> canonicalize workspace文件路径
    -> 读取UTF-8文本
    -> compile_str(source, canonical路径)

compile_str(source, workspace_file)
    -> serde_yaml反序列化WorkspaceFile；失败返回FileDecode
    -> 确定workspace_file父目录作为base
    -> build_definition(raw, base)

build_definition
    -> 校验Workspace名称
    -> project_root省略时使用base，否则按base解析为绝对路径
    -> 按agents配置顺序逐个处理
       -> 映射形式把键写入name，列表形式要求name字段
       -> 校验Agent名称
       -> 构造AgentImageReference，失败返回InvalidImageReference
       -> 将resources和disable_resources逐个构造成ResourceRef
       -> memory_path省略时保持None，否则相对project_root解析为绝对路径
    -> 空Agent列表失败
    -> 重复Agent名称失败
    -> manager省略时取第一个Agent名称
    -> manager不存在失败
    -> 返回WorkspaceDefinition

边界：
    Compose只解析引用和路径，不读取AgentImage、Skill、Workflow、models.toml或Memory
    Compose不创建World/Entity，不连接daemon，不发送网络请求
    CLI或protocol负责将WorkspaceDefinition转换为后端请求
    WorkspacePlugin仍对收到的WorkspaceDefinition执行运行时复核
```

## 持有关系

```text
Compose调用栈
├── source: &str（compile_str期间借用）
├── WorkspaceFile
│   ├── name
│   ├── project_root
│   ├── manager
│   └── AgentDocuments
│       └── Vec<RawAgent>
└── WorkspaceDefinition
    └── Vec<WorkspaceAgentDefinition>
        ├── AgentImageReference
        ├── Vec<ResourceRef>
        ├── Vec<ResourceRef>
        └── memory_path
```
