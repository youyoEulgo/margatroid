# SkillPlugin

SkillPlugin注册`tool:builtin/skill-loader:latest`，按完整`skill:scope/name:tag`查找`SKILL.md`。
查找顺序为项目、AgentImage、主目录；最高优先级目录存在但无效时直接失败。每次调用重新读取文件，
成功或失败都以关联原`tool_call_id`的`AgentMessage::Tool`返回。
