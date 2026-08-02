# 伪代码格式
```text
模块：使用一级标题，只写当前设计涉及的部分

类型：使用二级标题，按私有、crate公开和公开分组
TypeName：中文类型名，可见性类型--类型说明
    field_name: RustType--中文字段名，字段说明
    method_name<Generic>(self, parameter: ParameterType) -> ReturnType
        中文方法名：可见性方法，解释参数和用途
        约束：使用标准Rust where约束
        行为：展开完整逻辑
    impl TraitName for TypeName
        TraitName：可见性trait实现
        trait_method_name(&self, parameter: ParameterType) -> ReturnType
            中文方法名：解释参数和用途
            行为：标准库trait的简单行为用一句话说明

函数：使用二级标题，按私有、crate公开和公开分组，只放置不属于某个类型的操作
function_name<Generic>(parameter: ParameterType) -> ReturnType
    中文函数名：可见性函数，解释参数和用途
    约束：使用标准Rust where约束
    行为：展开完整逻辑

逻辑：使用二级标题，按执行顺序描述对象之间的调用关系
注释：字段注释使用--，类型、方法和函数的说明直接写在标题中
属性：不写Rust Attribute，实现时自行判断
边界：对外使用泛型和具体类型，内部使用类型擦除
```

# LogPlugin

## 类型

公开：
```text
LogError：日志错误，公开枚举--描述日志配置和进程级Subscriber安装错误
    InvalidFilter {
        filter: String--过滤器；无法解析的过滤表达式
        source: BoxedError--错误来源；类型擦除后的解析错误
    }
    InvalidMaxFiles {
        max_files: usize--最大文件数；无效的文件保留数量
    }
    InvalidStreamCapacity {
        capacity: usize--诊断流容量；无效的广播通道容量
    }
    FileOutputInitFailed {
        directory: PathBuf--目录；文件输出初始化失败的目标目录
        source: BoxedError--错误来源；类型擦除后的文件初始化错误
    }
    SubscriberAlreadyInstalled
    ConflictingConfiguration
    LogPluginAlreadyInstalled
    panic(self) -> !
        终止：crate公开方法，panic消费当前LogError，并使用Display文本终止执行
        行为：使用自身Display描述触发panic
    impl fmt::Display for LogError
        Display：公开trait实现
        fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result
            格式化错误：formatter接收稳定的错误描述
            行为：输出包含错误类型与上下文的稳定错误描述
    impl std::error::Error for LogError
        Error：公开trait实现
        source(&self) -> Option<&(dyn std::error::Error + 'static)>
            获取错误来源：返回配置解析或文件初始化的底层错误
            行为：InvalidFilter与FileOutputInitFailed返回内部类型擦除错误，其他变体返回None

LogLevel：日志级别，公开枚举
    Off
    Error
    Warn
    Info--默认级别
    Debug
    Trace
    directive(self) -> &'static str
        转换过滤指令：crate公开方法，根据当前日志级别返回tracing过滤指令
        行为：返回与当前级别对应的tracing过滤指令

LogFormat：日志格式，公开枚举
    Compact--默认格式
    Pretty
    Json

ConsoleTarget：Console输出目标，公开枚举
    Stdout
    Stderr--默认目标

LogRotation：日志轮转策略，公开枚举
    Minutely
    Hourly
    Daily--默认策略
    Never

FileLogOptions：文件日志配置，公开结构体
    directory: PathBuf--目录，crate公开
    file_name_prefix: String--文件名前缀，crate公开
    rotation: LogRotation--轮转策略，crate公开
    max_files: Option<usize>--最大文件数，crate公开，None表示不限制
    non_blocking: bool--是否非阻塞，crate公开
    daily<Path, Prefix>(directory: Path, prefix: Prefix) -> Self
        每日轮转：公开关联函数，directory指定日志目录，prefix指定文件名前缀
        约束：
            Path: Into<PathBuf>
            Prefix: Into<String>
        行为：使用指定目录和前缀构造每日轮转、无文件数上限、非阻塞的文件配置
    with_rotation(mut self, rotation: LogRotation) -> Self
        设置轮转：公开方法，rotation替换当前轮转策略
        行为：替换轮转策略并返回自身
    with_max_files(mut self, max_files: usize) -> Self
        设置最大文件数：公开方法，max_files指定需要保留的最大日志文件数量
        行为：max_files为0时终止并报告LogError::InvalidMaxFiles，否则保存最大文件数并返回自身
    blocking(mut self) -> Self
        使用阻塞写入：公开方法，将当前文件输出配置切换为阻塞Writer
        行为：将non_blocking设为false并返回自身

LogPlugin：日志插件，公开结构体--配置进程级tracing，并将EventLog汇入tracing
    level: LogLevel--级别；私有
    filter: Option<String>--过滤器；私有
    format: LogFormat--格式；私有
    console: Option<ConsoleTarget>--Console目标；私有，None时不创建Console Layer
    file: Option<FileLogOptions>--文件配置；私有，None时不创建File Layer
    stream_capacity: Option<usize>--诊断流容量；私有，None时不创建TracingStream Layer
    schedule: String--Schedule；私有，指定EventLog System所属Schedule
    with_level(mut self, level: LogLevel) -> Self
        设置级别：公开方法，level替换当前日志级别
        行为：保存level并返回自身
    with_filter<Filter>(mut self, filter: Filter) -> Self
        设置过滤器：公开泛型方法，filter提供可转换为String的tracing过滤表达式
        约束：Filter: Into<String>
        行为：转换filter，保存为Some并返回自身
    with_format(mut self, format: LogFormat) -> Self
        设置格式：公开方法，format替换Console和File输出格式
        行为：保存format并返回自身
    with_console(mut self, target: ConsoleTarget) -> Self
        设置Console：公开方法，target指定Console Layer写入stdout或stderr
        行为：将console设为Some(target)并返回自身
    without_console(mut self) -> Self
        关闭Console：公开方法，移除当前Console输出目标
        行为：将console设为None并返回自身
    with_file(mut self, options: FileLogOptions) -> Self
        设置文件输出：公开方法，options提供完整文件日志配置
        行为：将file设为Some(options)并返回自身
    with_stream(mut self, capacity: usize) -> Self
        设置诊断流：公开方法，capacity指定进程内广播通道容量
        行为：capacity为0时终止并报告LogError::InvalidStreamCapacity，否则保存容量并返回自身
    in_schedule<ScheduleName>(mut self, schedule: ScheduleName) -> Self
        指定Schedule：公开泛型方法，schedule提供EventLog System需要挂载的Schedule名称
        约束：ScheduleName: Into<String>
        行为：转换schedule，保存Schedule名称并返回自身
    impl Default for LogPlugin
        Default：公开trait实现
        default() -> Self
            构造默认Plugin：返回默认日志级别、格式、输出和Schedule配置
            行为：构造Info级、Compact格式、输出到Stderr、不启用文件与诊断流，并使用RuntimePlugin::POST_UPDATE
    impl Plugin for LogPlugin
        Plugin：公开trait实现
        build(self, app: &mut App)
            构建Plugin：self提供完整日志配置，app接收Subscriber、System和Resource配置
            行为：防止在同一App重复挂载，确认指定Schedule存在并完成日志配置校验，调用install_tracing完成进程级Subscriber安装，将event_log_system添加到指定Schedule，最后插入LogPluginInstalled

TracingField：tracing字段，公开结构体--表示一项结构化tracing字段
    name: String--字段名，公开
    value: String--字段值，公开

TracingRecord：tracing记录，公开结构体--TracingStream向外提供的结构化诊断记录
    timestamp_millis: u64--毫秒时间戳，公开
    level: String--级别，公开
    target: String--target，公开
    message: String--消息，公开
    fields: Vec<TracingField>--字段，公开
    spans: Vec<String>--Span栈，公开

TracingStream：诊断流，公开结构体--有界广播进程内tracing日志，可按target订阅SystemLog或EventLog
    sender: broadcast::Sender<TracingRecord>--发送端；私有
    new(capacity: usize) -> Self
        构造诊断流：crate公开关联函数，capacity指定广播通道容量
        行为：使用capacity创建广播通道并持有发送端
    layer(&self) -> TracingStreamLayer
        构造Layer：crate公开方法，克隆当前发送端并创建对应TracingStreamLayer
        行为：返回与当前诊断流共享发送端的TracingStreamLayer
    subscribe(&self) -> TracingSubscription
        订阅诊断流：公开方法，为调用者创建独立广播接收端
        行为：使用新接收端和初始丢弃数0构造TracingSubscription
    receiver_count(&self) -> usize
        获取接收者数量：公开方法，查询当前广播通道存活的接收端数量
        行为：返回当前广播接收端数量
    impl Resource for TracingStream
        Resource：公开trait实现

TracingSubscription：诊断流订阅，公开结构体--持有一个独立广播接收端及其累计丢弃数
    receiver: broadcast::Receiver<TracingRecord>--接收端；私有
    dropped: u64--已丢弃数；私有
    recv(&mut self) -> Result<TracingRecord, TracingStreamError>
        接收记录：公开异步方法，等待下一条TracingRecord，并报告通道关闭或接收者落后
        行为：成功时返回记录，通道关闭时返回Closed，落后时累加dropped并返回Lagged
    dropped_count(&self) -> u64
        获取已丢弃数：公开方法，读取当前订阅累计错过的记录数量
        行为：返回dropped

TracingStreamError：诊断流错误，公开枚举--允许后续版本增加错误变体
    Closed
    Lagged(u64)--丢弃记录数
    impl fmt::Display for TracingStreamError
        Display：公开trait实现
        fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result
            格式化诊断流错误：formatter接收关闭或丢弃数量描述
            行为：输出诊断流关闭或丢弃记录数
    impl std::error::Error for TracingStreamError
        Error：公开trait实现
```

私有：
```text
BoxedError：类型擦除错误，私有类型别名--等于Box<dyn std::error::Error + Send + Sync>

BoxedLayer：类型擦除Layer，私有类型别名--等于Box<dyn Layer<Registry> + Send + Sync>

INSTALL_LOCK：全局安装锁，私有静态变量，类型为Mutex<()>--串行化多个App或线程的Subscriber安装

INSTALLED_TRACING：全局已安装Tracing，私有静态变量，类型为OnceLock<InstalledTracing>--首个成功安装后保持到进程结束

InstalledTracing：已安装Tracing，私有结构体--由进程全局持有，保证配置与非阻塞Writer生命周期
    configuration: TracingConfiguration--配置
    stream: Option<TracingStream>--诊断流
    _worker_guards: Vec<WorkerGuard>--WorkerGuard；保持非阻塞Writer存活

TracingConfiguration：Tracing配置，私有结构体--只包含进程全局tracing配置，不包含App内Schedule
    level: LogLevel--级别
    filter: Option<String>--过滤器
    format: LogFormat--格式
    console: Option<ConsoleTarget>--Console目标
    file: Option<FileLogOptions>--文件配置
    stream_capacity: Option<usize>--诊断流容量

LogPluginInstalled：LogPlugin安装标记，私有结构体--表示LogPlugin已在当前App挂载的单元结构体
    impl Resource for LogPluginInstalled
        Resource：私有trait实现

FieldVisitor：字段访问器，私有结构体--收集tracing Event中的结构化字段
    fields: Vec<TracingField>--字段
    impl Visit for FieldVisitor
        Visit：私有trait实现
        record_debug(&mut self, field: &Field, value: &dyn fmt::Debug)
            记录Debug字段：field提供字段元数据，value提供字段值
            行为：将字段名和Debug值追加为TracingField
```

crate公开：
```text
TracingStreamLayer：诊断流Layer，crate公开结构体
    sender: broadcast::Sender<TracingRecord>--发送端
    impl<S> Layer<S> for TracingStreamLayer
        Layer：crate公开泛型trait实现
        约束：S: Subscriber + for<'lookup> LookupSpan<'lookup>
        on_event(&self, event: &tracing::Event<'_>, context: Context<'_, S>)
            处理tracing事件：event提供日志数据，context提供Span上下文
            行为：构造TracingRecord并尝试广播，无接收者时直接丢弃，慢接收者由广播通道报告Lagged且不阻塞日志调用

JsonLayer<Writer>：JSON Layer，crate公开结构体--将TracingRecord编码为单行JSON
    writer: Writer--Writer
    new(writer: Writer) -> Self
        构造JSON Layer：crate公开关联函数，writer提供每次事件使用的输出Writer
        行为：持有writer并构造JsonLayer
    impl<S, Writer> Layer<S> for JsonLayer<Writer>
        Layer：crate公开泛型trait实现
        约束：
            S: Subscriber + for<'lookup> LookupSpan<'lookup>
            Writer: for<'writer> tracing_subscriber::fmt::MakeWriter<'writer> + Send + Sync + 'static
        on_event(&self, event: &tracing::Event<'_>, context: Context<'_, S>)
            处理tracing事件：event提供日志数据，context提供Span上下文
            行为：将event转换为TracingRecord，编码为单行JSON并写入writer

```

## 函数

私有：
```text
install_tracing(plugin: &LogPlugin, app: &mut App)
    安装Tracing：私有函数，plugin提供进程级日志配置，app接收可选TracingStream资源
    行为：从plugin提取不含Schedule的TracingConfiguration，串行化全局安装，处理重复配置，构造全部Layer并安装Subscriber，保存配置、诊断流与WorkerGuard

build_filter(plugin: &LogPlugin) -> EnvFilter
    构造过滤器：私有函数，plugin提供显式filter或默认LogLevel
    行为：优先解析用户filter，否则解析LogLevel对应指令，解析失败时终止并报告LogError::InvalidFilter

console_layer(target: ConsoleTarget, format: LogFormat, plugin: &LogPlugin) -> BoxedLayer
    构造Console Layer：私有函数，target指定输出目标，format指定格式，plugin提供过滤配置
    行为：按target和format构造带过滤器的fmt Layer或JsonLayer

file_layer(options: &FileLogOptions, format: LogFormat, plugin: &LogPlugin) -> (BoxedLayer, Option<WorkerGuard>)
    构造File Layer：私有函数，options提供文件配置，format指定格式，plugin提供过滤配置
    行为：创建RollingFileAppender，失败时终止并报告LogError::FileOutputInitFailed；按配置选择阻塞或非阻塞Writer并构造Layer

format_file_layer<Writer>(format: LogFormat, writer: Writer, plugin: &LogPlugin) -> BoxedLayer
    格式化File Layer：私有泛型函数，format指定格式，writer接收日志，plugin提供过滤配置
    约束：Writer: for<'writer> tracing_subscriber::fmt::MakeWriter<'writer> + Send + Sync + 'static
    行为：按format构造禁用ANSI并带过滤器的fmt Layer或JsonLayer

record_from_event<S>(event: &tracing::Event<'_>, context: Context<'_, S>) -> TracingRecord
    从Event构造记录：私有泛型函数，event提供tracing事件，context提供当前Subscriber的Span上下文
    约束：S: Subscriber + for<'lookup> LookupSpan<'lookup>
    行为：读取时间、级别、target、message、字段和Span栈并构造TracingRecord
```

## 持有关系

```text
进程全局：
InstalledTracing--已安装Tracing
├── TracingConfiguration--Tracing配置
├── Option<TracingStream>--可选诊断流
│   └── broadcast::Sender<TracingRecord>--广播发送端
└── Vec<WorkerGuard>--WorkerGuard数组

tracing全局Dispatcher
└── Registry
    ├── Option<Console Layer>--可选Console Layer
    ├── Option<File Layer>--可选File Layer
    └── Option<TracingStreamLayer>--可选诊断流Layer

App
└── World
    └── Option<TracingStream> Resource--可选诊断流资源；与进程全局TracingStream共享广播发送端
```

## 逻辑

```text
首次挂载LogPlugin：
    -> 如果当前World已有LogPluginInstalled，终止并报告LogError::LogPluginAlreadyInstalled
    -> 调用App::contains_schedule确认指定schedule存在
    -> 如果schedule不存在，终止并报告CoreError::ScheduleNotFound
    -> 验证filter、file与stream_capacity
    -> 获取进程全局INSTALL_LOCK
    -> 根据配置创建Console、File和TracingStream Layer
    -> 调用tracing_subscriber::registry().with(layers).try_init()
    -> 如果进程已有外部Subscriber，终止并报告LogError::SubscriberAlreadyInstalled
    -> 将不含schedule的TracingConfiguration、TracingStream和WorkerGuard保存到INSTALLED_TRACING
    -> 如果启用诊断流，将TracingStream克隆作为Resource插入World
    -> 向指定schedule添加event_log_system
    -> 将LogPluginInstalled插入World

在另一个App挂载LogPlugin：
    -> 调用App::contains_schedule确认当前App的schedule存在
    -> 如果schedule不存在，终止并报告CoreError::ScheduleNotFound
    -> 验证filter、file与stream_capacity
    -> 获取进程全局INSTALL_LOCK
    -> 如果已安装TracingConfiguration与本次不同，终止并报告LogError::ConflictingConfiguration--schedule不参与比较
    -> 不重复安装Subscriber
    -> 如果启用诊断流，将同一个TracingStream克隆插入当前World
    -> 向当前App的schedule添加event_log_system
    -> 将LogPluginInstalled插入当前World

记录ECS诊断：
    -> 任意线程直接调用tracing宏
    -> 全局Dispatcher将Event交给Registry
    -> 每个启用的Layer独立处理同一个Event
    -> 不访问World，不创建ECS Event，不等待Schedule

约束：
    直接调用tracing宏生成的日志统称为SystemLog
    LogPlugin负责进程级tracing配置和EventLog到tracing的投影，但不独占EventLog的读取
    Subscriber每个进程只安装一次，首个成功配置在进程生命周期内不可替换
    默认配置只安装Console Layer，Console Layer同时处理SystemLog与EventLog
    用户显式配置的File Layer与TracingStream Layer也可按过滤规则处理EventLog
    非阻塞文件Writer的WorkerGuard由进程全局持有到进程结束
    TracingStream有界且不反向阻塞tracing调用
```

# EventLog

## 类型

公开：
```text
EVENT_LOG_TARGET：EventLog target，公开常量，类型为&str--固定为"mecs::event_log"

EventLog：事件日志，公开结构体--通过ECS事件队列传播
    level: LogLevel--级别，公开
    message: String--消息，公开
    new<Message>(level: LogLevel, message: Message) -> Self
        构造EventLog：公开泛型关联函数，level指定日志级别，message提供可转换为String的日志内容
        约束：Message: Into<String>
        行为：使用level和转换后的message构造EventLog
    impl Event for EventLog
        Event：公开trait实现

WorldEventLogExt：World事件日志扩展，公开trait--为World提供EventLog发送方法
    event_log<Message>(&self, level: LogLevel, message: Message)
        发送EventLog：公开泛型方法，level指定日志级别，message提供日志内容
        约束：Message: Into<String>
        行为：构造EventLog并调用WorldEventExt::send_event写入事件并唤醒Runtime

impl WorldEventLogExt for World
    WorldEventLogExt for World：公开trait实现--为World实现事件日志扩展
    event_log<Message>(&self, level: LogLevel, message: Message)
        发送EventLog：level指定日志级别，message提供日志内容
        约束：Message: Into<String>
        行为：调用WorldEventExt::send_event发送EventLog::new(level, message)
```

## System

crate公开：
```text
event_log_system(world: &mut World)
    处理EventLog：crate公开同步System，world提供本帧事件读取存储和tracing输出入口
    行为：
        读取本次更新的全部EventLog事件
        逐个分流EventLog.level
            Off：忽略
            Error：使用ERROR级别和EVENT_LOG_TARGET调用tracing Event
            Warn：使用WARN级别和EVENT_LOG_TARGET调用tracing Event
            Info：使用INFO级别和EVENT_LOG_TARGET调用tracing Event
            Debug：使用DEBUG级别和EVENT_LOG_TARGET调用tracing Event
            Trace：使用TRACE级别和EVENT_LOG_TARGET调用tracing Event
```

## 逻辑

```text
挂载顺序：
    -> 首先挂载RuntimePlugin创建默认Schedule
    -> 挂载LogPlugin安装进程级tracing，并将event_log_system默认添加到POST_UPDATE
    -> 再挂载其他会在build期间输出tracing日志的Plugin

发送EventLog：
    -> System调用WorldEventLogExt::event_log
    -> EventLog进入core事件队列并唤醒Runtime
    -> 下一次tick自动建立或装填EventLog读取存储
    -> POST_UPDATE中的event_log_system读取事件
    -> 使用固定EVENT_LOG_TARGET转换为tracing Event
    -> 已安装的tracing Layer按过滤规则输出

HttpPlugin处理EventLog：
    -> HttpPlugin向自己指定的Schedule添加http_event_log_system
    -> http_event_log_system独立读取同一帧的EventLog
    -> 按HttpPlugin的配置过滤并通过HTTP广播
    -> 不修改tracing Subscriber，不影响event_log_system的读取

约束：
    EventLog必须经过ECS事件队列，不能由System直接调用tracing宏替代
    LogPlugin中的event_log_system只负责事件到tracing Event的转换，不保存其他业务状态
    EventLog可以被多个System独立读取，一个System的处理不会消费或屏蔽其他System的读取
    请求ID、Agent ID和其他上下文暂不由LogPlugin定义，需要时扩展EventLog而不是解析message
```
