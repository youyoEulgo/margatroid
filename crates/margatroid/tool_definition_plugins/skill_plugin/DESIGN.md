# SkillPlugin

## 类型

公开：
```text
SkillPlugin：Skill注册与执行Plugin，公开结构体
    home_root: Arc<PathBuf>--主目录Skill根
    open(home_root: impl Into<PathBuf>) -> Result<Self, SkillError>
    impl Plugin for SkillPlugin
        构建：安装SkillRegisterRequest和ToolCallRequest处理System；不在安装时注册具体Skill或全局Loader模板

SkillRegisterRequest：Agent Skill注册请求，公开事件
    id: String--Workspace注册子请求ID
    agent: Entity--目标Agent Entity，必须已挂载AgentToolMap和AgentToolEnvironment
    resource_id: ResourceId--待注册完整Skill ID
    impl Event for SkillRegisterRequest

SkillRegisterResponse：Agent Skill注册结果，公开事件
    id: String--原注册子请求ID
    agent: Entity
    resource_id: ResourceId
    result: Result<(), ToolError>
    impl Event for SkillRegisterResponse

SkillError：Skill配置错误，公开结构体
SkillErrorKind：Skill配置错误分类，公开枚举
```

私有：
```text
SkillRoots：Skill根配置，私有Resource
    home_root: Arc<PathBuf>

SkillArguments：Skill调用参数，私有结构体--当前无字段；只接受JSON对象

SkillMetadata：Skill TOML元信息，私有结构体
    name: String--非空Skill名称
    description: String--非空ToolSpec描述

SkillDocument：已解析Skill文档，私有结构体
    metadata: SkillMetadata
    body: String--不含frontmatter的非空Markdown正文
```

## 函数

```text
skill_register_system(world: &mut World)
    注册Skill：私有System，读取SkillRegisterRequest
    行为：
        验证resource_id使用type=skill及受支持tag
        从Agent Entity读取AgentToolEnvironment
        按项目、镜像、主目录顺序精确查找SKILL.md
        解析顶部+++包围的TOML元信息并构造Provider无关ToolTemplate；description来自frontmatter，不把Skill正文放入模板
        调用ToolPlugin注册接口写入当前Agent的AgentToolMap
        tool_id固定为tool:builtin/skill-loader:latest，resource_id保持具体Skill ID
        成功或失败都发送SkillRegisterResponse

skill_tool_call_system(world: &mut World)
    执行Skill：私有System，读取ToolCallRequest
    行为：
        只处理tool_id=tool:builtin/skill-loader:latest
        使用request.resource_id重新按项目、镜像、主目录查找并解析SKILL.md
        解析request.arguments；当前只接受JSON对象，无参数为"{}"
        成功时发送ToolCallResponse { turn_id, agent, tool_call_id, result: Ok(不含frontmatter的Markdown正文) }
        失败时发送同定位信息和稳定ToolError
        不自行发送AgentMessage

find_skill_file(environment: &AgentToolEnvironment, home_root: &Path, resource_id: &ResourceId) -> Result<PathBuf, ToolError>
    查找Skill：项目、镜像、主目录顺序；找到目录但缺少SKILL.md时立即失败，不回退

read_skill_document(path: &Path) -> Result<SkillDocument, ToolError>
    解析Skill：严格要求文件以+++开头并以第二个+++结束TOML区域
    行为：使用TOML解析name与description，拒绝未知字段、空字段和空正文
```

## 逻辑

```text
Workspace启动：
    WorkspacePlugin -> SkillRegisterRequest
    SkillPlugin -> 验证并读取元信息
                -> register_agent_tool(agent, skill-loader, resource_id, template)
                -> SkillRegisterResponse

每次调用：
    ToolPlugin -> ToolCallRequest { tool_id=skill-loader, resource_id=具体Skill }
    SkillPlugin -> 重新读取当前SKILL.md
                -> ToolCallResponse
    ToolPlugin -> 根据ToolCallResponse整理为AgentMessage::Tool
```

## 边界

```text
SkillPlugin不读取Agent可见性，不保存每个Skill的全局注册项，不维护pending调用，不自行构造AgentMessage。
Skill正文每次调用重新读取，保证loading skill每轮得到当前内容；正文不进入AgentToolMap的ToolTemplate。
注册过程不伪造ToolCall，不保存test_arguments，不执行有副作用的测试操作。
SkillPlugin只负责Skill资源验证、模板构造、SKILL.md读取和ToolCallResponse。
```
