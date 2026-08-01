# 伪代码格式
```text
模块：使用一级标题，只写当前设计涉及的部分

类型：使用二级标题，按私有和公开分组
类型名：类型种类--类型说明
    字段名：字段类型--字段说明
    方法名：可见性方法
        签名：method(参数名：参数类型) -> 返回类型--无返回值时省略箭头和返回类型
        行为：自定义方法展开完整逻辑
    trait实现：可见性trait实现
        签名：Trait<关联类型> for 类型
        行为：标准库trait的行为用一句话说明

函数：使用二级标题，按私有和公开分组，只放置不属于某个类型的操作
函数名：可见性函数
    签名：function(参数名：参数类型) -> 返回类型--无返回值时省略箭头和返回类型
    行为：展开完整逻辑

逻辑：使用二级标题，按执行顺序描述对象之间的调用关系
注释：统一使用--，可以附在对象后或单独成行
边界：对外使用泛型和具体类型，内部使用类型擦除
```

# LogPlugin

## 类型

公开：
```text
LogError：枚举--描述日志配置和进程级Subscriber安装错误
    NonExhaustive：公开属性--允许后续版本增加错误变体
    InvalidFilter { filter: 字符串, source: 类型擦除错误 }
    InvalidMaxFiles { max_files: 无符号整数 }
    InvalidStreamCapacity { capacity: 无符号整数 }
    FileOutputInitFailed { directory: 路径, source: 类型擦除错误 }
    SubscriberAlreadyInstalled
    ConflictingConfiguration
    LogPluginAlreadyInstalled
    终止：crate公开方法
        签名：panic(self) -> Never
        行为：使用自身Display描述触发panic
    Debug：公开trait实现
        签名：Debug for LogError
    Display：公开trait实现
        签名：Display for LogError
        行为：输出包含错误类型与上下文的稳定错误描述
    Error：公开trait实现
        签名：Error for LogError
        行为：InvalidFilter与FileOutputInitFailed返回内部类型擦除错误作为source，其他变体不返回source

LogLevel：枚举
    Off
    Error
    Warn
    Info--默认
    Debug
    Trace
    转换过滤指令：crate公开方法
        签名：directive(self) -> 静态字符串引用
        行为：返回与当前级别对应的tracing过滤指令
    标准特征：公开trait实现
        签名：Clone + Copy + Debug + PartialEq + Eq + Default for LogLevel

LogFormat：枚举
    Compact--默认
    Pretty
    Json
    标准特征：公开trait实现
        签名：Clone + Copy + Debug + PartialEq + Eq + Default for LogFormat

ConsoleTarget：枚举
    Stdout
    Stderr--默认
    标准特征：公开trait实现
        签名：Clone + Copy + Debug + PartialEq + Eq + Default for ConsoleTarget

LogRotation：枚举
    Minutely
    Hourly
    Daily--默认
    Never
    标准特征：公开trait实现
        签名：Clone + Copy + Debug + PartialEq + Eq + Default for LogRotation

FileLogOptions：结构体
    目录：路径--私有
    文件名前缀：字符串--私有
    轮转策略：LogRotation--私有
    最大文件数：可选<无符号整数>--私有
    是否非阻塞：布尔值--私有
    每日轮转：公开泛型方法
        签名：daily<Path, Prefix>(directory: Path, prefix: Prefix) -> Self
        约束：Path可转换为路径，Prefix可转换为字符串
        行为：使用指定目录和前缀构造每日轮转、无文件数上限、非阻塞的文件配置
    设置轮转：公开方法
        签名：with_rotation(self, rotation: LogRotation) -> Self
        行为：替换轮转策略并返回自身
    设置最大文件数：公开方法
        签名：with_max_files(self, max_files: 无符号整数) -> Self
        行为：max_files为0时终止并报告LogError::InvalidMaxFiles，否则设置最大文件数并返回自身
    使用阻塞写入：公开方法
        签名：blocking(self) -> Self
        行为：将是否非阻塞设为假并返回自身
    标准特征：公开trait实现
        签名：Clone + Debug + PartialEq + Eq for FileLogOptions

LogPlugin：结构体--配置进程级tracing，并将EventLog汇入tracing
    级别：LogLevel--私有
    过滤器：可选<字符串>--私有
    格式：LogFormat--私有
    Console目标：可选<ConsoleTarget>--私有，为空时不创建Console Layer
    文件配置：可选<FileLogOptions>--私有，为空时不创建File Layer
    诊断流容量：可选<无符号整数>--私有，为空时不创建TracingStream Layer
    Schedule：字符串--私有，EventLog System所属Schedule
    设置级别：公开方法
        签名：with_level(self, level: LogLevel) -> Self
        行为：替换级别并返回自身
    设置过滤器：公开泛型方法
        签名：with_filter<Filter>(self, filter: Filter) -> Self
        约束：Filter可转换为字符串
        行为：设置过滤器并返回自身
    设置格式：公开方法
        签名：with_format(self, format: LogFormat) -> Self
        行为：替换格式并返回自身
    设置Console：公开方法
        签名：with_console(self, target: ConsoleTarget) -> Self
        行为：设置Console目标并返回自身
    关闭Console：公开方法
        签名：without_console(self) -> Self
        行为：清空Console目标并返回自身
    设置文件输出：公开方法
        签名：with_file(self, options: FileLogOptions) -> Self
        行为：设置文件配置并返回自身
    设置诊断流：公开方法
        签名：with_stream(self, capacity: 无符号整数) -> Self
        行为：capacity为0时终止并报告LogError::InvalidStreamCapacity，否则设置诊断流容量并返回自身
    指定Schedule：公开泛型方法
        签名：in_schedule<ScheduleName>(self, schedule: ScheduleName) -> Self
        约束：ScheduleName可转换为字符串
        行为：替换EventLog System所属Schedule并返回自身
    默认：公开trait实现
        签名：Default for LogPlugin
        行为：构造Info级别、Compact格式、输出到Stderr、不启用文件与诊断流，并使用RuntimePlugin::POST_UPDATE的LogPlugin
    Plugin：公开trait实现
        签名：Plugin for LogPlugin
        行为：防止在同一App重复挂载，确认指定Schedule存在并完成日志配置校验，调用install_tracing完成进程级Subscriber安装，注册EventLog，将event_log_system添加到指定Schedule，启用诊断流时将TracingStream作为Resource插入World，最后插入LogPluginInstalled
    标准特征：公开trait实现
        签名：Clone + Debug + PartialEq + Eq for LogPlugin

TracingField：结构体--一项结构化tracing字段
    name: 公开字符串
    value: 公开字符串
    标准特征：公开trait实现
        签名：Clone + Debug + PartialEq + Eq + Serialize + Deserialize for TracingField

TracingRecord：结构体--TracingStream向外提供的结构化诊断记录
    timestamp_millis: 公开64位无符号整数
    level: 公开字符串
    target: 公开字符串
    message: 公开字符串
    fields: 公开数组<TracingField>
    spans: 公开数组<字符串>
    标准特征：公开trait实现
        签名：Clone + Debug + PartialEq + Eq + Serialize + Deserialize for TracingRecord

TracingStream：结构体--有界广播进程内tracing日志，可按target订阅SystemLog或EventLog
    发送端：广播发送端<TracingRecord>--私有
    构造：crate公开方法
        签名：new(capacity: 无符号整数) -> Self
        行为：使用指定容量创建广播通道并持有发送端
    构造Layer：crate公开方法
        签名：layer(&self) -> TracingStreamLayer
        行为：克隆广播发送端并构造TracingStreamLayer
    订阅：公开方法
        签名：subscribe(&self) -> TracingSubscription
        行为：创建新的广播接收端，以丢弃数0构造TracingSubscription
    接收者数量：公开方法
        签名：receiver_count(&self) -> 无符号整数
        行为：返回当前广播接收端数量
    Clone：公开trait实现
        签名：Clone for TracingStream
    Resource：公开trait实现
        签名：Resource for TracingStream

TracingSubscription：结构体
    接收端：广播接收端<TracingRecord>--私有
    已丢弃数：64位无符号整数--私有
    接收：公开异步方法
        签名：recv(&mut self) -> Result<TracingRecord, TracingStreamError>
        行为：成功时返回记录，通道关闭时返回Closed，落后时累加丢弃数并返回Lagged
    已丢弃数：公开方法
        签名：dropped_count(&self) -> 64位无符号整数
        行为：返回累计丢弃记录数

TracingStreamError：枚举
    NonExhaustive：公开属性--允许后续版本增加错误变体
    Closed
    Lagged(64位无符号整数)
    Debug：公开trait实现
        签名：Debug for TracingStreamError
    Display：公开trait实现
        签名：Display for TracingStreamError
        行为：输出诊断流关闭或丢弃记录数
    Error：公开trait实现
        签名：Error for TracingStreamError
```

私有：
```text
类型擦除Layer：类型别名<Box<dyn Layer<Registry> + Send + Sync>>

全局安装锁：静态互斥锁<单元>--串行化多个App或线程的Subscriber安装

全局已安装Tracing：静态OnceLock<已安装Tracing>--首个成功安装后保持到进程结束

已安装Tracing：结构体--进程全局持有，保证配置与非阻塞Writer生命周期
    配置：TracingConfiguration
    诊断流：可选<TracingStream>
    WorkerGuard：数组<WorkerGuard>

TracingConfiguration：结构体--仅包含进程全局tracing配置，不包含App内Schedule
    级别：LogLevel
    过滤器：可选<字符串>
    格式：LogFormat
    Console目标：可选<ConsoleTarget>
    文件配置：可选<FileLogOptions>
    诊断流容量：可选<无符号整数>
    标准特征：私有trait实现
        签名：Clone + Debug + PartialEq + Eq for TracingConfiguration

LogPluginInstalled：单元结构体--标记LogPlugin已在当前App挂载
    Resource：私有trait实现
        签名：Resource for LogPluginInstalled

TracingStreamLayer：结构体
    发送端：广播发送端<TracingRecord>
    Layer：私有泛型trait实现
        签名：Layer<Subscriber类型> for TracingStreamLayer
        行为：每次收到tracing Event时构造TracingRecord并尝试广播，无接收者时直接丢弃，慢接收者由广播通道报告Lagged且不阻塞日志调用

JsonLayer<Writer>：结构体
    Writer：Writer
    构造：私有方法
        签名：new(writer: Writer) -> Self
        行为：持有Writer并构造JsonLayer
    Layer：私有泛型trait实现
        签名：Layer<Subscriber类型> for JsonLayer<Writer>
        行为：将tracing Event转换为TracingRecord，编码为单行JSON并写入Writer

FieldVisitor：结构体
    字段：数组<TracingField>
    Default：私有trait实现
        签名：Default for FieldVisitor
    Visit：私有trait实现
        签名：Visit for FieldVisitor
        行为：将tracing字段名和Debug值追加为TracingField
```

## 函数

私有：
```text
安装Tracing：私有函数
    签名：install_tracing(plugin: LogPlugin只读引用, app: App可变引用)
    行为：从plugin提取不含Schedule的TracingConfiguration，串行化全局安装，处理重复配置，构造全部Layer并安装Subscriber，保存配置、诊断流与WorkerGuard

构造过滤器：私有函数
    签名：build_filter(plugin: LogPlugin只读引用) -> EnvFilter
    行为：优先解析用户过滤器，否则解析LogLevel对应指令，解析失败时终止并报告LogError::InvalidFilter

构造Console Layer：私有函数
    签名：console_layer(target: ConsoleTarget, format: LogFormat, plugin: LogPlugin只读引用) -> 类型擦除Layer
    行为：按目标和格式构造带过滤器的fmt Layer或JsonLayer

构造File Layer：私有函数
    签名：file_layer(options: FileLogOptions只读引用, format: LogFormat, plugin: LogPlugin只读引用) -> (类型擦除Layer, 可选<WorkerGuard>)
    行为：创建滚动文件Appender，失败时终止并报告LogError::FileOutputInitFailed；按配置选择阻塞或非阻塞Writer并构造Layer

格式化File Layer：私有泛型函数
    签名：format_file_layer<Writer>(format: LogFormat, writer: Writer, plugin: LogPlugin只读引用) -> 类型擦除Layer
    行为：按格式构造禁用ANSI并带过滤器的fmt Layer或JsonLayer

从Event构造记录：私有泛型函数
    签名：record_from_event<Subscriber类型>(event: tracing Event只读引用, context: Layer Context) -> TracingRecord
    行为：读取时间、级别、target、message、字段和Span栈并构造TracingRecord
```

## 持有关系

```text
进程全局：
已安装Tracing
├── TracingConfiguration
├── 可选TracingStream
│   └── 广播发送端<TracingRecord>
└── WorkerGuard数组

tracing全局Dispatcher
└── Registry
    ├── 可选Console Layer
    ├── 可选File Layer
    └── 可选TracingStreamLayer

App
└── World
    └── 可选TracingStream资源--与进程全局TracingStream共享广播发送端
```

## 逻辑

```text
首次挂载LogPlugin：
    -> 如果当前World已有LogPluginInstalled，终止并报告LogError::LogPluginAlreadyInstalled
    -> 调用App::contains_schedule确认指定Schedule存在
    -> 如果Schedule不存在，终止并报告CoreError::ScheduleNotFound
    -> 验证过滤器、文件配置与诊断流容量
    -> 获取进程全局安装锁
    -> 根据配置创建Console、File和TracingStream Layer
    -> 调用tracing_subscriber::registry().with(layers).try_init()
    -> 如果进程已有外部Subscriber，终止并报告LogError::SubscriberAlreadyInstalled
    -> 将不含Schedule的TracingConfiguration、TracingStream和WorkerGuard保存到进程全局已安装Tracing
    -> 如果启用诊断流，将TracingStream克隆作为Resource插入World
    -> 注册EventLog并向指定Schedule添加event_log_system
    -> 将LogPluginInstalled插入World

在另一个App挂载LogPlugin：
    -> 调用App::contains_schedule确认当前App的指定Schedule存在
    -> 如果Schedule不存在，终止并报告CoreError::ScheduleNotFound
    -> 验证过滤器、文件配置与诊断流容量
    -> 获取进程全局安装锁
    -> 如果已安装Tracing的TracingConfiguration与本次不同，终止并报告LogError::ConflictingConfiguration--Schedule不参与比较
    -> 不重复安装Subscriber
    -> 如果启用诊断流，将同一个TracingStream克隆插入当前World
    -> 注册EventLog并向当前App的指定Schedule添加event_log_system
    -> 将LogPluginInstalled插入当前World

记录ECS诊断：
    -> 任意线程直接调用tracing宏
    -> 全局Dispatcher将Event交给Registry
    -> 每个启用的Layer独立处理同一个Event
    -> 不访问World，不创建ECS Event，不等待Schedule

约束：
    直接调用tracing宏生成的日志统称为SystemLog
    LogPlugin负责进程级tracing配置、EventLog注册和EventLog到tracing的投影，但不独占EventLog的读取
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
EVENT_LOG_TARGET：静态字符串常量 = "mecs::event_log"

EventLog：结构体--通过ECS事件队列传播的日志
    level: 公开LogLevel
    message: 公开字符串
    构造：公开泛型方法
        签名：new<Message>(level: LogLevel, message: Message) -> Self
        约束：Message可转换为字符串
        行为：使用level和转换后的message构造EventLog
    Event：公开trait实现
        签名：Event for EventLog
    Debug：公开trait实现
        签名：Debug for EventLog

WorldEventLogExt：trait--由LogPlugin为World提供EventLog发送方法
    发送EventLog：公开泛型方法
        签名：event_log<Message>(&self, level: LogLevel, message: Message)
        约束：Message可转换为字符串
        行为：构造EventLog并调用WorldEventExt::send_event写入事件并唤醒Runtime
```

## System

私有：
```text
EventLog System：同步System
    签名：event_log_system(world: World可变引用)
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
    -> 挂载LogPlugin安装进程级tracing，并将EventLog System默认添加到POST_UPDATE
    -> 再挂载其他会在build期间输出tracing日志的Plugin

发送EventLog：
    -> System调用WorldEventLogExt::event_log
    -> EventLog进入core事件队列
    -> 下一次tick装填EventLog读取存储
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
