# 后端装配与生命周期

`build_app` 装配后端，`shutdown_app` 等待实际任务、连接和网络服务收口。构建源码不会替换正在运行的 daemon，也不会迁移或改写已有用户数据。

Compose 位于业务 plugin 之上，负责装配多个 plugin 和 core；领域不得反向依赖 Compose。配置读取归各领域，业务执行与文件操作不集中到装配层。中止控制固定在 PRE_UPDATE，MCL、推理和工具在 UPDATE 或 POST_UPDATE 受理，先清除失效 turn，再处理后续请求。

## 创建和启动

Workspace 使用绝对节点路径，例如 `/demo`，管理目录为数据根下的 `workspaces/demo`。`workspace.create` 创建 `workspace.sqlite` 和 `agents/normal`、`temporary`、`archived` 三类目录，并挂载 Workspace 实体；`project_root` 是独立的实际项目目录，不可与管理目录互相包含。

Agent 启动先调用 Driver 工具。`agent.start` 的 `new` 变体传入 Workspace、实际工具清单路径和完整参数；`existing` 变体只传 Workspace 和已登记节点名，从数据库读取原工具路径和参数。节点名、别名及工作目录参数由 Driver 使用，不另设宿主隐式参数通道。

Driver 的 `margatroid.agent:create` 统一检查节点的 Driver 和父节点归属，取得节点操作凭证，准备文件并打开存储连接。随后提交登记，再把已连接的 `AgentStorage`、实际父实体和任务 ID 交给 Agent System。System 同步挂载 Agent、地址、MCL 队列、工具集合、任务集合、推理句柄和存储组件，将返回的关闭能力绑定到创建任务。

普通 Agent 的节点名在同一 Workspace 中唯一；相同 Driver、相同父节点可复用未运行记录，其他 Driver 不能接管。临时 Agent 每次分配独立记录、目录及运行节点。

## 关闭、归档和退出

`agent.stop` 从目标 Agent 的 `parent` 和 `task_id` 定位创建任务。任务关闭要等待全部绑定 Agent，Agent 关闭又要等待自身 MCL、推理、子任务、来源监控和存储物理关闭。Agent 的关闭句柄不会反向关闭创建者任务。一个任务创建多个 Agent 时，停止该任务会关闭全部绑定实例。

创建取消后仍由独立监督完成已受理的创建交接及补偿；关闭失败保留未完成清理能力，显式重试不重新执行 Driver 业务。

`agent.archive` 与同节点创建互斥，先关闭实际创建任务，再移动整个数据目录并提交归档类别。数据库内的归档意图记录移动中断的原、目标位置，后续显式重试或启动加载继续完成；不会用未知目录覆盖已有数据，不进行旧结构自动迁移。

正常应用退出关闭受理门，继续推进 World，等待初始化加载、已受理 Workspace 创建及 Agent 创建交接，再逐个关闭任务、实体和数据库。退出保留持久目录，不调用删除事件；已受理的显式删除继续使用原监督和回复完成。

## 同步与监控

组件变化和集合修改通过统一探针通知。Workspace 数据库只在确认提交后发布登记快照；历史、MCL 和日志用真实提交序号唤醒记录监控。UI 和配置监控观察实际来源，关闭或替换时等待在途读取完成。所有网络来源共享同一个 Clients 槽位，不增加同步中转事件或逐帧扫描。

API 契约和信息流拓扑位于独立的 `doc/api` 仓库。实体地址采用绝对节点路径，文件资源标识继续使用 `type:scope/name:tag`。旧 Workspace YAML 和已删除的网络接口没有兼容入口。

## 验证

真实 Driver 进程测试需要先构建运行器：

```bash
cargo build -p margatroid
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

测试可用 `MARGATROID_TEST_RUNNER` 指定已构建的同版本运行器。测试使用独立临时目录和临时端口，不更新或重启已部署服务。前端源码和构建产物由前端开发者适配，此实现不修改它们。
