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
        ├── setting.toml
        └── state.sqlite.blocks/
            └── <block>/
                ├── timeline.sqlite
                └── snapshot_<id>.sqlite
```

启动读取全局配置、模型路由及Workspace定义展示缓存，不自动启动所有Workspace。每次Workspace启动或重启请求携带path，后端重读文件、更新展示缓存并直接使用本次解析结果；禁止缓存兜底启动。

Workspace拥有专属线程，Agent和Lua分别管理生命周期和VM退出；停止遵守活动真实收口、VM退出、线程join的顺序。进程退出或连接断开不自动等价于业务清理完成。

DTO原生WebSocket读取后提交DtoRequest，再由专属中止事件保留两帧控制路径。Inference在单通道出口累积并推流，中止受理冻结内容，真实清理后才回复。前端静态资源编译期嵌入，修改UI后必须重新构建并编译daemon。


Ctrl+C和SIGTERM先唤醒并停止正常运行循环，随后调用Compose关停；World继续推进，直到Workspace、Agent、推理、工具VM、子进程及后代、管道和Axum全部收口后，daemon才返回终端。信号监听保持到清理结束，重复Ctrl+C不跳过清理，也不设置超时后强退。清理失败报告诊断并继续监督；已完成领域清理后的Axum运行错误仍以非零状态退出。SIGKILL或操作系统强制终止不经过这条路径。
