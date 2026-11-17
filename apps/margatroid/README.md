# Margatroid

margatroid是统一应用入口，定位~/.margatroid，调用Compose装配并运行App。配置读取由各领域负责，daemon不处理协议或定义业务System。

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

定义加载目标统一为全量重载并发布缓存，不自动启动所有Workspace。Workspace及Agent启动/重启前先完成同一全量重载，再从本次缓存快照选择定义及成员参数；重载失败不启动，不以旧缓存兜底，也不单独回源某个Workspace。

Workspace拥有专属线程，Agent和Lua分别管理生命周期和VM退出；停止遵守活动真实收口、VM退出、线程join的顺序。进程退出或连接断开不自动等价于业务清理完成。

DTO原生WebSocket读取后提交DtoRequest，回合中止通过专属控制事件直接进入PRE_UPDATE。Inference在单通道出口累积并推流，中止受理冻结内容，真实清理后才回复。前端静态资源编译期嵌入，修改UI后必须重新构建并编译daemon。


Ctrl+C和SIGTERM先唤醒并停止正常运行循环，随后调用Compose关停；World继续推进，直到Workspace、Agent、推理、工具VM、子进程及后代、管道和Axum全部收口后，daemon才返回终端。信号监听保持到清理结束，重复Ctrl+C不跳过清理，也不设置超时后强退。清理失败报告诊断并继续监督；已完成领域清理后的Axum运行错误仍以非零状态退出。SIGKILL或操作系统强制终止不经过这条路径。

启动时向终端stderr输出数据目录；Axum实际绑定成功后输出启动成功、真实监听地址、前端HTTP URL及WebSocket URL。配置端口为0时显示操作系统分配的实际端口。监听0.0.0.0或::时，浏览器链接分别使用127.0.0.1或[::1]，监听行仍显示实际绑定地址。启动提示直接输出，独立于业务日志等级及筛选配置。

```text
margatroid: starting (data directory: /home/example/.margatroid)
margatroid: started successfully (listening on 127.0.0.1:3939)
Webui:  http://127.0.0.1:3939/
WebSocket: ws://127.0.0.1:3939/ws
```

绑定失败时结束正常帧循环，执行既有清理并以非零状态返回实际错误。成功标识只在真实监听之后打印一次。

应用退出覆盖所有Workspace状态：Starting/Running/Error均提交原StopWorkspace；Stopping等待已有监督及其原reply，不重复关闭。Starting中的Lua立即收到取消，仍等待初始化和进程真实退出。回归直接在启动失败后发送SIGINT/SIGTERM，不预先调用workspace.stop，并验证未完成初始化的Lua子进程在daemon退出前已回收。

未绑定Agent的日志直接通过既有tracing输出，遵守配置的日志级别和过滤器；不创建独立运行日志数据库、不强制保存运行级报告。绑定Agent的日志沿原AgentStorage及提交探针处理。

唯一主程序包和编译产物均名为margatroid，入口位于apps/margatroid，不再构建旧CLI工作区控制客户端或单独daemon程序。使用cargo build --release -p margatroid，再运行./target/release/margatroid启动服务；工具worker/supervisor仍由同一可执行文件内部模式启动。mcp-client和shell-client是工具辅助程序，继续独立保留。
