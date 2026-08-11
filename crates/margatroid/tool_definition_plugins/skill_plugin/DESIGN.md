# SkillPlugin

## 定位

SkillPlugin是`provider="skill"`的`ToolDefinitionProvider`。每个Skill资源都被解析成一个独立
`Tool`，与普通Tool走完全相同的构造链路。

不存在全局`use_skill(name)`工具，也不在工具参数中进行第二次资源路由。Skill的`ResourceRef`就是
可见性单元，模型可见工具名直接对应这一个Skill。

## 类型

公开：
```text
SkillPlugin：Skill工具定义Plugin，公开结构体--配置主目录Skill根并注册SkillToolProvider
    home_root: Arc<PathBuf>--主目录Skill根，例如~/.margatroid/skills
    open(home_root: impl Into<PathBuf>) -> Result<Self, SkillError>
        打开Plugin：规范化并保存绝对主目录根，不扫描资源库
    impl Plugin for SkillPlugin
        build(self, app: &mut App)
            构建插件：公开trait方法，通过ToolPlugin注册provider ID skill的SkillToolProvider

SkillErrorKind：Skill Plugin配置错误分类，公开枚举
    InvalidRoot--主目录Skill根不是无父级跳转的绝对路径

SkillError：Skill Plugin配置错误，公开结构体
    kind: SkillErrorKind--稳定错误分类，私有
    message: String--不泄露Skill正文的描述，私有
    new(kind: SkillErrorKind, message: impl Into<String>) -> Self
        构造错误：私有关联函数，保存配置错误描述
    kind(&self) -> SkillErrorKind
        取得分类：公开方法
    message(&self) -> &str
        取得描述：公开方法
    impl Display + Error for SkillError
```

私有：
```text
SkillToolProvider：Skill工具定义提供方，私有结构体
    home_root: Arc<PathBuf>
    impl ToolDefinitionProvider for SkillToolProvider
        id() -> "skill"
        provide(environment, name) -> Result<Tool, ToolError>
            按项目、AgentImage、主目录顺序查找并读取SKILL.md
            ToolDefinition.description使用完整SKILL.md正文
            handler忽略参数并返回完整SKILL.md正文
```

## 函数

私有：
```text
find_skill_file(environment, home_root, name) -> Result<PathBuf, ToolError>
    查找Skill文件：按项目目录、AgentImage目录、主目录顺序返回第一个存在的SKILL.md

exposed_name(name: &ResourceName) -> Result<String, ToolError>
    构造模型名称：生成skill_<scope>_<name>并限制为ToolPlugin允许的64字节

normalize_root(path: PathBuf) -> Option<PathBuf>
    规范化主目录根：要求绝对路径、拒绝父级跳转并去除当前目录段
```

## 逻辑

```text
AgentDynamicVisibility包含：
    ResourceRef { provider: "skill", name: local/code-review }

准备LLM请求：
    ToolPlugin按provider="skill"找到SkillToolProvider
        -> 依次查找项目、AgentImage和主目录中的local/code-review/SKILL.md
        -> 读取最高优先级SKILL.md正文
        -> provider为local/code-review构造一个Tool
        -> Tool.resource保持同一个ResourceRef
        -> ToolDefinition.name使用skill_local_code-review
        -> ToolDefinition.description使用SKILL.md正文
        -> AgentPlugin将definition加入InferenceCommand.tools

模型调用Skill：
    ToolPlugin执行Skill Tool handler
        -> 忽略当前JSON object参数
        -> 返回SKILL.md正文作为Message::Tool
        -> AgentPlugin把Tool消息加入实时tool_context并继续推理
        -> AgentPlugin另外在历史事件中写入"skill: <scope/name> loaded"，不写入正文
```

最高优先级同名目录存在但内容非法时直接失败，不向低优先级降级。SkillPlugin不读取Agent可见性；
它只为ToolPlugin传入的单个ResourceRef提供定义和执行器。当前不解析frontmatter、不执行脚本、不读取
Skill目录中的其他资源。

Skill动态加载：AgentStatus只保存工具调用模板，每轮由AgentPlugin重新生成调用ID并调用ToolPlugin；SkillPlugin每次执行都按优先级重新读取SKILL.md，不缓存正文。
