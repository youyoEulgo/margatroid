# SkillPlugin

SkillPlugin注册`provider="skill"`的`ToolDefinitionProvider`。每个可见Skill都是一个独立Tool，
不存在额外的`use_skill(name)`路由。是否进入模型请求完全由Agent动态可见性决定。

