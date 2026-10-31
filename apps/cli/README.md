# Margatroid CLI

```text
margatroid workspace up [NAME] [--backend WS_URL]
```

CLI注册为cli，通过state.query取得Workspace路径和展示缓存。名称精确选择配置；省略名称时匹配规范当前目录和project_root，单项直接选择、多项交互选择，非交互多项拒绝。远程后端使用显式名称，不假设两端文件系统一致。

启动发送workspace.start，message只含path，id在外层信封；不上传缓存定义、YAML、revision或数据库路径。后端重新加载该文件决定本次启动，缓存仅帮助选择。CLI不扫描服务端目录、不创建项目配置或状态文件。

匹配workspace.started后保持连接显示日志，成员失败独立显示。Ctrl+C或SIGTERM请求停止已启动Workspace，等待workspace.stopped；启动中收到信号保留停止意图，成功后再停止。失败、断线及等待超时报告实际结果，不声称后端已自动取消。

stdin仅用于启动前的工作区选择；等待期间继续处理心跳和日志。CLI不新增Agent或Workspace重启命令，不提供配置修改或重载入口。
