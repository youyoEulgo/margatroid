本文为共享Client槽位及同步入口/监控的目标装配设计；本轮尚未实现，状态见README。

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
# handler src/handler.rs  插件构造与安装
# types   src/types.rs    应用路径及装配参数
# error   src/error.rs    装配错误
```

# lib

## 函数

公开：
```text
build_app(options: ApplicationOptions) -> Result<App, ComposeError>
    行为：构造插件、读取启动配置、安装基础设施和业务插件，返回未进入运行循环的App。
```

# handler

## 函数

私有：
```text
prepare(options: &ApplicationOptions) -> Result<ApplicationParts, ComposeError>
    行为：先读取ConfigPlugin，创建唯一Clients并调用create_sync_bindings，再把Workspace绑定交给WorkspacePlugin::open，调用InferencePlugin::open，准备资源根及runner路径；将config自有tools经resolve_tool_limits补齐并校验预算，失败映射config领域及config.toml路径，先于App创建和插件安装；任一失败返回，不自动启动工作区。

resolve_tool_limits(config: &FileToolConfig) -> Result<ToolExecutionLimits, ConfigError>
    行为：将lua_vm_memory_limit和task_output_buffer_window直接映射到lua_vm_memory_bytes和cache_bytes；缺省值只取工具领域默认2GiB及128MiB，两项拒绝显式零值并报告tools字段路径；lua_vm_memory_bytes只限制工具Lua VM分配器，cache_bytes独立约束任务输出窗口，不限制Rust或外部进程，不限制返回结果容量。

install(app: &mut App, parts: ApplicationParts) -> Result<(), ComposeError>
    行为：先安装既有mecs Runtime、AsyncRuntime、Closure和日志设施，再安装Margatroid axum_plugin及各业务插件。
        复用prepare已创建的唯一Clients Arc槽位和dto_plugin::create_sync_bindings返回的typed绑定，不在install再次创建。
        把同一Clients注入ClientPlugin及DtoPlugin，把各自core定义的Workspace/Agent/Task/UI/Storage/Inference同步绑定分别注入对应插件及生命周期上下文。
        领域plugin只调用自身或其他领域core入口，不依赖DTO plugin；DTO函数返回的绑定由compose组装，wire编码函数通过owned回调注入。
        将已校验Lua预算通过ToolPlugin::with_limits注入工具插件，runner路径通过with_runner_path传入。
        不注册统一通用HTTP/WebSocket事件路由；bind参数交AxumPlugin，ingress由config_core拥有并由DTO handler直接校验。
        Client共享槽位同时拥有连接身份及sender，World与Axum持有同一Arc；取消ConnState副本和长期固定sender快照，发送源每次直接筛选当前Client。
        DtoPlugin在build中自行注册/ws原生Router及自有State，UiPlugin自行注册静态Router；Compose不再手工调用UiPlugin::router后add_http_routes。
        各插件使用AppAxumExt::add_axum_routes提交已完成with_state的Router；所有插件build完成后STARTUP由普通send_async_closure运行整个Axum服务，ECS继续推进。
        handler可直接返回Response、使用专用channel/oneshot或发送自己的事件，不要求统一通用网络事件。
        AxumPlugin仅作为领域基础设施允许DTO/UI依赖；业务plugin之间仍不得互相依赖。
        安装时固定abort_control_system于RuntimePlugin::PRE_UPDATE；mcl_command_system、inference_system和tool_execute_system只允许UPDATE或POST_UPDATE，拒绝PRE_UPDATE及无法证明顺序的自定义schedule；不新增调度metadata，不修改mecs调度实现。
        同帧中止先清turn，再取消已受理推理及尚未return工具；之后到达业务System的旧turn事件必须拒绝，不能先执行再回查。
        各生命周期按Stop、Restart、Start顺序挂载；启动后创建Workspace定义监控，Agent实际来源/存储安装时启动UI和记录监控，替换/停止时取消并等待真实join。
        不安装dto_sync_system，不创建WebSocketMessageSend或sync中转通道，不用活动帧轮询获得更新；业务命令、生命周期及中止控制的原事件链保持。
        9种sync的wire名称和字段保持，状态源初始化及更新使用每连接原有队列，有序记录和chunk不作状态覆盖。

shutdown_app(app: &mut App) -> Future<Output = Result<(), ComposeError>>
    行为：
        应用退出先关闭外部新业务受理并发出Axum停止信号，保持ECS与异步执行器运行，以允许在途业务、Workspace和连接处理完成。
        先停止新监控受理，取消Workspace磁盘监控；请求Workspace/Agent按原生命周期真实停止，失效旧来源并等待UI、记录及实际blocking读取全部join。
        等待工具、推理、MCL及VM真实退出；移除Client共享槽位、取消连接并等待其在途快照prepare及socket writer结束。
        等待AxumHandle::wait_stopped，确认普通请求和已追踪升级工作均已退出，再释放运行设施；先停止ECS循环再等待依赖其回复的handler属于无效关停顺序。
        任一清理无法确认时返回诊断并保留监督，不把drop App、关闭监听端口或丢弃Future当安全完成。

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
    clients: Clients
    sync_bindings: SyncBindings
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
