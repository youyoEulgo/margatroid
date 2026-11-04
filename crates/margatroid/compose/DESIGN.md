# 伪代码格式

```text
板块使用一级标题，一行注明文件，无内容不建空模块。
类型、函数使用二级标题，列出字段、签名及直接行为。
Resource写在lib板块；持有关系在文末使用树形图。
不写独立逻辑段、进度、计划或验证记录。
```

# 设计范围

Margatroid领域使用原生Axum装配及普通异步闭包，已退出mecs通用网络桥和spawn_async_service依赖。mecs现有接口、文档及源码暂时保留，后续另行清理，不提供旧入口回退或兼容实现。

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
    行为：调用ConfigPlugin::open、WorkspacePlugin::open和InferencePlugin::open；准备资源根及runner路径；任一失败返回，不自动启动工作区。

install(app: &mut App, parts: ApplicationParts) -> Result<(), ComposeError>
    行为：先安装既有mecs Runtime、AsyncRuntime、Closure和日志设施，再安装Margatroid axum_plugin及各业务插件。
        不安装mecs ServerPlugin或注册其通用HTTP/WebSocket事件路由；bind参数交AxumPlugin，ingress由config_core拥有并由DTO handler直接校验。
        Client先提供ConnectionId、ClientFrame及ClientSender，DTO/Inference只依赖client_core的发送能力，不依赖server_plugin类型。
        DtoPlugin在build中自行注册/ws原生Router及自有State，UiPlugin自行注册静态Router；Compose不再手工调用UiPlugin::router后add_http_routes。
        各插件使用AppAxumExt::add_axum_routes提交已完成with_state的Router；所有插件build完成后STARTUP由普通send_async_closure运行整个Axum服务，ECS继续推进。
        handler可直接返回Response、使用专用channel/oneshot或发送自己的事件，不要求统一通用网络事件。
        AxumPlugin仅作为领域基础设施允许DTO/UI依赖；业务plugin之间仍不得互相依赖。
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
