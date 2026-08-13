# SkillPlugin

## 类型

公开：
```text
SkillPlugin：Skill工具定义Plugin，公开结构体--注册skill Loader并处理Skill路由事件
    home_root: Arc<PathBuf>--主目录Skill根
    open(home_root: impl Into<PathBuf>) -> Result<Self, SkillError>
    impl Plugin for SkillPlugin

SkillError：Skill配置错误，公开结构体
SkillErrorKind：Skill配置错误分类，公开枚举
```

私有：
```text
SkillLoaderTemplate：Skill Loader模板，私有常量定义
    id: tool:builtin/skill-loader:latest
    definition.name: skill_loader--仅注册表内部模板名，不进入InferenceRequest
    definition.input_schema: {"type":"object"}--模型不负责传Skill ResourceId

Skill路径查找函数：按project/.margatroid/skills、image/skills、home顺序查找
```

## 函数

```text
skill_tool_definition_system(world: &mut World)
    定义检查：读取ToolDefinitionRoute
    行为：只处理skill-loader；验证skill:scope/name:tag，精确查找SKILL.md；成功返回name=完整Skill ResourceId且无参数的ToolDefinitionResult，失败发送错误结果

skill_tool_call_system(world: &mut World)
    Skill调用：读取ToolCallEvent
    行为：只处理skill-loader；按event.resource中的完整ResourceId重新查找并读取SKILL.md；成功或失败都直接发送AgentMessage::Tool，保留原轮次ID和tool_call_id

find_skill_file(environment: &AgentToolEnvironment, home_root: &Path, resource: &ResourceId) -> Result<PathBuf, ToolError>
    查找Skill：project、image、home顺序；找到目录但缺少SKILL.md时立即失败，不回退
```

## 逻辑

```text
SkillPlugin不读取可见性，不保存每个Skill注册项，不把Skill正文放入ToolRegistry。
Skill正文按每次定义检查和每次调用重新读取；当前只读取SKILL.md，Workflow和脚本执行留待后续设计。
Skill工具响应仍是AgentMessage::Tool；历史记录由AgentPlugin按skill资源写入标记文本，不写正文。
```
