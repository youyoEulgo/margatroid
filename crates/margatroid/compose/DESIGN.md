本文为共享Client槽位及统一同步入口/监控的实际装配设计；运行进程需使用相应构建。

# 伪代码格式

```text
板块使用一级标题，一行注明文件，无内容不建空模块。
类型、函数使用二级标题，列出字段、签名及直接行为。
Resource写在lib板块；持有关系在文末使用树形图。
不写独立逻辑段、进度、计划或验证记录。
```

# 设计范围

Margatroid领域使用原生Axum装配及普通异步闭包。网络服务属于本应用基础设施；mecs仅提供通用运行与异步事件、闭包能力，不提供服务器封装或长期任务专用接口。

# 板块格式

```text
# lib     src/lib.rs      应用装配入口
# handler src/handler.rs  插件构造、持久诊断与安装
# types   src/types.rs    应用路径及装配参数
# error   src/error.rs    装配错误
```

# lib

## 函数

公开：
```text
build_app(options: ApplicationOptions) -> Result<App, ComposeError>
    行为：构造插件、读取启动配置、安装基础设施和业务插件，返回未进入运行循环的App。
shutdown_application(app: &mut App) -> Future<Output = Result<(), ComposeError>>
    行为：调用shutdown_app；统一使用任务句柄与实际关闭结果，不发送不存在的StopWorkspace事件，不调用DeleteWorkspace删除数据。

```

# handler

## 函数

私有：
```text
prepare(options: &ApplicationOptions) -> Result<ApplicationParts, ComposeError>
    行为：读取ConfigPlugin与InferencePlugin::open，按数据根构造WorkspacePlugin { root }，准备资源根及runner路径；将config自有tools经resolve_tool_limits补齐并校验预算，失败映射config领域及config.toml路径，先于App创建和插件安装；任一失败返回，不自动启动工作区。

resolve_tool_limits(config: &FileToolConfig) -> Result<ToolExecutionLimits, ConfigError>
    行为：将lua_vm_memory_limit和task_output_buffer_window直接映射到lua_vm_memory_bytes和cache_bytes；缺省值只取工具领域默认2GiB及128MiB，两项拒绝显式零值并报告tools字段路径；lua_vm_memory_bytes只限制工具Lua VM分配器，cache_bytes独立约束任务输出窗口，不限制Rust或外部进程，不限制返回结果容量。

install(app: &mut App, parts: ApplicationParts) -> Result<(), ComposeError>
    行为：先安装既有mecs Runtime、AsyncRuntime、Closure和日志设施，再安装Margatroid axum_plugin及各业务插件。
        install先创建唯一Clients，只调用一次dto_plugin::sync_bindings(clients.clone())，取得共享视图衍生的通用绑定包；不分别创建孤立的AgentViews。
        将唯一Clients、WorkspaceUpdateContext、AgentUpdateContext以及推理和沙箱core定义的ObserverFactory作为Resource插入同一World；网络探针和集合绑定状态由DTO plugin持有，其他plugin只取得core通用入口或装配方注入的回调，不依赖DTO或资源plugin的类型。
        ClientPlugin和DtoPlugin复用同一Clients Resource；WorkspaceUpdateContext与AgentUpdateContext保存core定义的通用回调，由DTO构造，安装前注入World。
        Tools::new()与ToolTasks::new()保持无参独立构造；绑定包内部生命周期回调在实际Installed时绑定工具/任务，ToolPlugin同时接收tasks通用回调用于非Agent对象，按集合身份幂等去重。
        领域plugin只调用自身或其他领域core入口，不依赖DTO plugin；DTO函数返回的绑定由compose组装，wire编码函数通过owned回调注入。
        将已校验Lua预算通过ToolPlugin::with_limits注入工具插件，runner路径通过with_runner_path传入，绑定包tasks回调通过with_task_observer传入。
        不注册统一通用HTTP/WebSocket事件路由；bind参数交AxumPlugin，ingress由config_core拥有并由DTO handler直接校验。
        Client共享槽位同时拥有连接身份及sender，World与Axum持有同一Arc；取消ConnState副本和长期固定sender快照，发送源每次直接筛选当前Client。
        DtoPlugin在build中自行注册/ws原生Router及自有State，UiPlugin自行注册静态Router；Compose不再手工调用UiPlugin::router后add_http_routes。
        各插件使用AppAxumExt::add_axum_routes提交已完成with_state的Router；所有插件build完成后STARTUP由普通send_async_closure运行整个Axum服务，ECS继续推进。
        handler可直接返回Response、使用专用channel/oneshot或发送自己的事件，不要求统一通用网络事件。
        AxumPlugin仅作为领域基础设施允许DTO/UI依赖；业务plugin之间仍不得互相依赖。
        安装时固定abort_control_system于RuntimePlugin::PRE_UPDATE；mcl_command_system、inference_system和tool_execute_system只允许UPDATE或POST_UPDATE，拒绝PRE_UPDATE及无法证明顺序的自定义schedule；不新增调度metadata，不修改mecs调度实现。
        同帧中止先清turn，再取消已受理推理及尚未return工具；之后到达业务System的旧turn事件必须拒绝，不能先执行再回查。
        Workspace仅在初始化schedule扫描目录；Installed探针绑定其实际数据库提交watch，不再安装Workspace YAML监控。创建和删除提交及Workspace状态变化沿各自统一入口更新公共视图。
        Agent Installed绑定组件和工具/任务集合探针；实际UI及存储SourcesInstalled后建立UI、设置和已提交记录的异步监控，SourcesClosing取消并等待真实monitor/worker退出，再退休视图。
        所有绑定通过core通用回调或compose注入，不让任何业务plugin直接调用其他业务plugin。
        不安装dto_sync_system，不创建WebSocketMessageSend或sync中转通道，不用活动帧轮询获得更新；业务命令、生命周期及中止控制的原事件链保持。
        现有API目录的sync类别保持；状态源按版本覆盖，有序记录和chunk不作状态覆盖，不因补充探针另增网络事件。

shutdown_app(app: &mut App) -> Future<Output = Result<(), ComposeError>>
    行为：关闭外部新业务受理并请求Axum停止；保持World和AsyncRuntime推进。
        World阶段逐个Workspace关闭database及ToolTasks的受理门，克隆所有实际工具任务及在途操作等待；不删除Workspace或Agent持久目录。
        World外等待任务finish、Agent创建交接和子Agent关闭，等database.wait_idle；任何失败保留实体与任务监督并返回错误，允许显式重试。
        调用Workspace探针Closing失效来源，再关闭实际数据库连接；watch收到None后结束，monitor的真实退出由AxumWorkGuard跟踪，不借用World或数据库连接。
        World阶段移除已清理Workspace并notify(Removed)，最终Axum停止等待也覆盖这些monitor guard；正常退出不删除目录。普通退出不发DeleteWorkspace，不删除文件。
        等所有UI、配置、记录、模型监控和客户端prepare worker退出，再等待socket writer及AxumHandle真实停止，最后释放World运行设施。

```

# types

## 类型

公开：
```text
ApplicationOptions：应用装配参数
    data_root: PathBuf
    runner_path: PathBuf
```

私有：
```text
ApplicationParts：安装前材料--已构造插件、Axum监听参数、ingress配置及日志过滤
    tool_limits: ToolExecutionLimits--已补齐默认值并校验的工具预算
```

# error

## 类型

公开：
```text
ComposeError：装配错误--保留失败领域、文件路径及原错误，不输出凭据
    impl Display + std::error::Error
```

# 持有关系

```text
Daemon
└── build_app
    ├── ApplicationOptions
    ├── mecs Runtime / AsyncRuntime / Closure / Log（本阶段不修改）
    ├── Margatroid AxumPlugin
    │   └── Router集合、AxumHandle及共享退出跟踪
    └── 各领域Plugin
        ├── DtoPlugin--/ws原生handler，自有协议/连接State
        ├── UiPlugin--静态资源handler，直接响应
        └── 各自core--业务状态、专用事件、通道与退出责任

应用退出编排
├── 关闭新受理，通知Axum和Workspace停止
├── 保持World继续处理在途事件和异步闭包结果
├── 等待领域真实收口及Axum/升级工作退出
└── 最后释放mecs设施
```
