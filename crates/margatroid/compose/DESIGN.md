本文描述本轮源码实现的中止与ToolTask生命周期契约；不表示用户已部署或正在运行的daemon已更新。

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
    行为：调用ConfigPlugin::open、WorkspacePlugin::open和InferencePlugin::open，准备资源根及runner路径；将config自有tools经resolve_tool_limits补齐并校验预算，失败映射config领域及config.toml路径，先于App创建和插件安装；任一失败返回，不自动启动工作区。

resolve_tool_limits(config: &FileToolConfig) -> Result<ToolExecutionLimits, ConfigError>
    行为：将total_task_memory_limit、task_memory_limit、task_output_buffer_window直接映射到total_memory_bytes、memory_bytes、cache_bytes；缺省值只取工具领域默认8GiB、2GiB、128MiB，三项均拒绝显式零值并报告tools字段路径；允许单任务内存大于总额，执行时受较小值限制；总额覆盖同一daemon所有Agent工具任务，任务内存组含工具worker VM及所有子孙进程并排除宿主Driver VM与supervisor，输出窗口按任务滚动保存且不计入任务内存组或限制返回结果容量。

install(app: &mut App, parts: ApplicationParts) -> Result<(), ComposeError>
    行为：先安装既有mecs Runtime、AsyncRuntime、Closure和日志设施，再安装Margatroid axum_plugin及各业务插件。
        将已校验预算通过ToolPlugin::with_limits注入工具插件，runner路径通过with_runner_path传入。
        不注册统一通用HTTP/WebSocket事件路由；bind参数交AxumPlugin，ingress由config_core拥有并由DTO handler直接校验。
        Client先提供ConnectionId、ClientFrame及ClientSender，DTO/Inference只依赖client_core的发送能力，不依赖服务器框架类型。
        DtoPlugin在build中自行注册/ws原生Router及自有State，UiPlugin自行注册静态Router；Compose不再手工调用UiPlugin::router后add_http_routes。
        各插件使用AppAxumExt::add_axum_routes提交已完成with_state的Router；所有插件build完成后STARTUP由普通send_async_closure运行整个Axum服务，ECS继续推进。
        handler可直接返回Response、使用专用channel/oneshot或发送自己的事件，不要求统一通用网络事件。
        AxumPlugin仅作为领域基础设施允许DTO/UI依赖；业务plugin之间仍不得互相依赖。
        安装时固定abort_control_system于RuntimePlugin::PRE_UPDATE；mcl_command_system、inference_system和tool_execute_system只允许UPDATE或POST_UPDATE，拒绝PRE_UPDATE及无法证明顺序的自定义schedule；不新增调度metadata，不修改mecs调度实现。
        同帧中止先清turn，再取消已受理推理及尚未return工具；之后到达业务System的旧turn事件必须拒绝，不能先执行再回查。
        各生命周期按Stop、Restart、Start顺序挂载；外部/ws信封、请求ID、状态及历史数据契约保持原义，不在本次实现Driver WebUI。

shutdown_application(app: &mut App) -> Future<Output = Result<(), ComposeError>>
    行为：
        应用退出先关闭外部新业务受理并发出Axum停止信号，保持ECS与异步执行器运行，以允许在途业务、Workspace和连接处理完成。
        请求Workspace/Agent按原生命周期真实停止，等待工具、推理、MCL及VM退出；DTO连接观察共享停止信号并结束各自读写及升级Future。
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
