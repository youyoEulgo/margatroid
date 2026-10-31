# Margatroid Daemon

Daemon是应用入口，定位~/.margatroid，调用Compose装配并运行App。配置读取由各领域负责，daemon不处理协议或定义业务System。

```text
~/.margatroid/
├── config.toml
├── models.toml
├── agent-drivers/
├── tools/
├── prompts/
├── sandboxes/
└── workspaces/<workspace>/
    ├── workspace.yaml
    └── agents/<agent>/
        ├── state.sqlite
        └── settings.toml
```

启动读取全局配置、模型路由及Workspace定义展示缓存，不自动启动所有Workspace。每次Workspace启动或重启请求携带path，后端重读文件、更新展示缓存并直接使用本次解析结果；禁止缓存兜底启动。

Workspace拥有专属线程，Agent和Lua分别管理生命周期和VM退出；停止遵守活动真实收口、VM退出、线程join的顺序。进程退出或连接断开不自动等价于业务清理完成。

网络继续使用Axum到通用事件、DTO到专属中止事件的两帧路径。Inference在单通道出口累积并推流，中止受理冻结内容，真实清理后才回复。前端静态资源编译期嵌入，修改UI后必须重新构建并编译daemon。
