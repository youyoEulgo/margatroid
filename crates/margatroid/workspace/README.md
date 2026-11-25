# Workspace

Workspace 的权威持久信息保存在 `workspaces/<节点名>/workspace.sqlite`。运行组件只持有 project_root、state 和数据库句柄，不另存展示别名；实体地址由 Address 组件提供。具体结构和函数见 [DESIGN.md](DESIGN.md)。

## 加载与生命周期

启动时扫描管理根中的 Workspace 目录，读取严格版本数据库并恢复已记录的归档意图，再在 World 挂载 Workspace。状态为 Running、Deleting、Error；创建、删除、Driver 启动及 Agent 归档沿现有事件与异步闭包执行，不建立专属 Workspace 执行线程。

创建要求管理目录不存在、项目根为实际目录且与管理目录互不包含。删除先关闭受理门，等待在途操作和所属任务真实清理，关闭数据库，再删除管理目录；项目根不删除。错误保留可诊断状态，不以取消或丢弃 Future 代替真实完成。

## Agent 登记与启动

数据库记录 id、父节点、普通节点名、别名、目录类别、Driver 工具路径及完整调用参数。普通、临时、归档类别与 Agent 运行状态独立；未运行和归档记录仍可查询和修改展示别名。

Existing 启动以已登记的工具入口和上次实际调用参数为基础，用当前登记的 node_name 和 alias 重新整理参数；alias 已清空时移除旧 alias。其他 Driver 自定义参数保持原值，New 仍原样使用调用参数。Driver 创建注入方法在操作凭证保护下准备文件、连接 AgentStorage、提交登记并等待 StartAgent。已有记录的 alias 始终以数据库列为准，不接受启动参数反向覆盖；tool_arguments 只在实际登记提交时更新为本次调用记录，失败前不预写。普通目录为 `agents/normal/<node_name>`；临时目录使用独立 id；归档移动到新分配的归档目录，保留 previous_node_name 和 previous_alias。

## 数据库提交快照与别名

数据库是别名的唯一权威来源。WorkspaceRecordsSnapshot 在同一版本中提供 metadata（address、alias、project_root）与全部 Agent 登记；metadata() 克隆这份确认快照，不读取启动时固定副本。登记、归档及别名事务均在 COMMIT 后发布完整快照，内容不变不递增 revision。

`rename_workspace_alias(alias)` 只改 Workspace 的展示别名；`rename_agent_alias(agent_id, alias)` 按持久 id 修改当前 Agent 别名，None 表示清空。非空别名必须非全白、无控制字符，保留原始前后空白、不要求唯一。Agent 节点、地址、目录、Driver 参数和 previous_alias 不变，不重启运行实例。

修改使用数据库操作受理凭证，Agent 别名与创建、归档共用目标操作键，冲突返回 Busy。已受理写入计入 wait_idle，关闭必须等待真实事务结束；关闭受理后拒绝新写入。找不到 Agent 记录返回 AgentRecordMissing；HTTP/WS 边界要求 Workspace 仍为 Running。

DTO 从同一确认快照取得 Workspace 和 Agent 别名，经原有 workspace.sync 发布，不新增 alias 组件、别名同步事件或逐帧数据库查询。World 读取不执行 SQL；磁盘 SQLite/WAL 文件事件不能替代数据库提交通知。

## 边界

core 只依赖 mecs 与通用库，通过通用回调提供生命周期通知；网络和客户端投影留在 plugin。没有 YAML 定义缓存、旧资源地址 Workspace、旧目录迁移或兼容重命名入口。`workspace.rename` 是展示别名修改，不是节点或目录重命名。

源码和隔离测试更新不表示用户数据库已被修改，也不表示正在运行的 daemon 已采用新构建。
