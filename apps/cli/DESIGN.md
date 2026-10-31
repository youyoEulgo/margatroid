# 伪代码格式

```text
板块使用一级标题，一行注明文件；类型和函数使用二级标题。
只写接口、字段和直接行为，不写独立逻辑段或过程记录。
持有关系在文末使用树形图。
```

# 板块格式

```text
# lib     src/main.rs   参数解析与CLI入口
# handler src/main.rs   注册、选择、请求和回执处理
# types   src/main.rs   CLI参数及连接内状态
# error   src/error.rs  CLI错误
```

# lib

## 函数

私有：
```text
main()
    行为：解析workspace up参数，执行run并输出退出码。
```

# handler

## 函数

私有：
```text
run(command: WorkspaceCommand) -> Future<Output = Result<(), CliError>>
    行为：连接WebSocket，注册cli，发送state.query；按名称或当前目录选择Workspace路径，发送workspace.start并等待匹配回执；持续显示日志，退出信号发送workspace.stop并等待关闭结果。

choose_workspace(definitions: Vec<WorkspaceConfigDto>, name: Option<String>, cwd: PathBuf) -> Result<PathBuf, CliError>
    行为：显式名称精确匹配；否则按缓存project_root与规范cwd相等筛选；零项报错，一项直接选择，多项交互选择，非交互多项拒绝。

handle_server_message(message: ServerMessage, session: &mut CliSession)
    行为：按网络id匹配注册、状态和生命周期回执；处理日志及Ping/Pong；成员启动失败单独显示，不误判整个Workspace未启动。
```

# types

## 类型

私有：
```text
WorkspaceCommand：命令参数
    Up { name: Option<String>, backend: String }

CliSession：连接内状态
    pending_id: Option<String>
    workspace_path: Option<PathBuf>
    workspace_address: Option<String>
    stop_requested: bool
```

# error

## 类型

私有：
```text
CliError：CLI错误--参数、连接、选择、超时及后端请求错误
    impl Display + std::error::Error
```

# 持有关系

```text
CLI进程
├── WorkspaceCommand
└── CliSession
    ├── WebSocket连接
    ├── Workspace展示配置
    └── 网络请求关联
```
