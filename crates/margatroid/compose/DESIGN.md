# Compose

## 统一身份与归属

```text
输出类型定义于共享types的workspace_components模块：
    WorkspaceDefinition { id, name, manager, project_root, agents }
    WorkspaceAgentDefinition { id, name, image, home_root, history_path }
地址统一使用ResourceID纯值，完整格式type:scope/name:tag，缺省标签latest。
Compose只验证地址语法，不解析资源正文，不创建World或Entity。
```

## 类型

公开：
```text
ComposeErrorKind：编译错误分类
    FileRead
    FileDecode
    InvalidDefinition
    InvalidPath
    InvalidName
    InvalidImageReference
    InvalidResourceReference
    DuplicateAgent
    MissingManager

ComposeError：有界错误，公开值类型
    kind: ComposeErrorKind--私有分类
    message: String--私有描述，最多512字节，UTF-8边界截断
    kind(&self) -> ComposeErrorKind
    message(&self) -> &str
    impl Clone + Debug + PartialEq + Eq + Display + Error
```

私有：
```text
WorkspaceFile：拒绝未知字段的YAML顶层文档
    name: String--必填逻辑名称
    project_root: Option<PathBuf>--相对配置文件目录的项目根
    manager: Option<String>--默认入口成员名称
    agents: AgentDocuments--必填，可为空

AgentDocuments：接受映射或序列并保留出现顺序
    0: Vec<RawAgent>
    Deserialize：映射键填入name；内部name若存在必须匹配键
        序列保留条目，name必填检查在构造阶段执行

RawAgent：拒绝未知字段的成员输入
    name: Option<String>--映射由键提供，序列必须显式提供
    image: String--必填镜像地址
    home_root: Option<PathBuf>--缺省读取真实HOME
    history_path: Option<PathBuf>--输入别名memory_path
    不接受tag、resources、disable_resources、tool_default或tool_dynamic字段
```

## 函数

```text
compile(path: impl AsRef<Path>) -> Result<WorkspaceDefinition, ComposeError>
    canonicalize文件路径，读取UTF-8 YAML，调用compile_str
    路径读取失败返回FileRead

compile_str(source: &str, workspace_file: impl AsRef<Path>) -> Result<WorkspaceDefinition, ComposeError>
    反序列化WorkspaceFile；缺字段、未知字段或无效YAML返回FileDecode
    将workspace_file转绝对路径，以其父目录作为base，调用build_definition

build_definition(raw: WorkspaceFile, base: &Path) -> Result<WorkspaceDefinition, ComposeError>
    校验Workspace名称
    project_root省略时使用base，显式路径按base解析
    按原顺序逐个构造成员：
        要求并校验name
        image若无image:前缀则补前缀，解析为type=image的ResourceID
        id固定为agent:<workspace-name>/<agent-name>:latest，无成员tag字段
        home_root显式路径相对project_root；省略时读取并规范化真实HOME
        history_path显式路径相对project_root；省略时生成
            <project_root>/.margatroid/history/<agent-name>.sqlite
    拒绝重复成员名称
    非空成员：manager缺省第一个成员，要求合法且存在
    空成员：manager缺省为空；显式空串合法，非空manager返回MissingManager
    生成workspace:local/<workspace-name>:latest，返回完整静态定义

validate_logical_name(value: &str, kind: &str) -> Result<(), ComposeError>
    非空、最多128字节、不是点或双点、不含斜杠反斜杠与控制字符
    地址构造仍由ResourceID复核其字段约束

absolute_path(path: &Path) -> Result<PathBuf, ComposeError>
    相对输入先按当前目录拼接，再normalize_path

resolve_path(base: &Path, path: PathBuf) -> Result<PathBuf, ComposeError>
    相对路径按base拼接，绝对路径直接规范化

normalize_path(path: PathBuf) -> Result<PathBuf, ComposeError>
    要求绝对路径，移除CurDir，拒绝任何ParentDir
```

## 边界与生命周期

```text
Compose不读取镜像、工具、Skill、模型路由或历史正文，不打开数据库。
Compose不创建资源默认列表，不执行资源IMPORT，不维护可见性列表。
memory_path只是一种输入拼写，不引入第二份Memory路径字段。
输出只保存纯地址、名称与路径，无ECS Entity或运行期线程能力。
CLI/protocol将定义发至后端，Workspace插件再执行运行期校验和生命周期管理。
空工作区仍由Workspace插件创建专属执行线程，不等首个成员才创建。
```
