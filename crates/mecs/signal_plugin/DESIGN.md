# 伪代码格式
```text
模块：使用一级标题，只写当前设计涉及的部分

类型：使用二级标题，按私有、crate公开和公开分组
TypeName：中文类型名，可见性类型--类型说明
    field_name: RustType--中文字段名，字段说明
    method_name<Generic>(self, parameter: ParameterType) -> ReturnType
        中文方法名：可见性方法，解释参数和用途
        约束：使用标准Rust where约束；单个约束写在同一行
        行为：展开完整逻辑
    impl TraitName for TypeName
        TraitName：可见性trait实现
        trait_method_name(&self, parameter: ParameterType) -> ReturnType
            中文方法名：解释参数和用途
            行为：标准库trait的简单行为用一句话说明

函数：使用二级标题，按私有、crate公开和公开分组，只放置不属于某个类型的操作
function_name<Generic>(parameter: ParameterType) -> ReturnType
    中文函数名：可见性函数，解释参数和用途
    约束：使用标准Rust where约束；单个约束写在同一行
    行为：展开完整逻辑

逻辑：使用二级标题，按执行顺序描述对象之间的调用关系
注释：字段注释使用--，类型、方法和函数的说明直接写在标题中
属性：不写Rust Attribute，实现时自行判断
边界：对外使用泛型和具体类型，内部使用类型擦除
```

# SignalPlugin

设计原则：只将操作系统信号转换为ECS事件，不解释信号的业务含义，不决定Runtime是否关闭。

crate名称：signal_plugin。

## 类型

公开：
```text
ProcessSignal：进程信号，公开枚举--提供跨平台稳定的进程信号语义
    Interrupt
    Terminate
    Hangup
    Quit
    WindowChanged
    User1
    User2
    Raw(i32)--仅Unix公开

ProcessSignalReceived：收到进程信号，公开结构体--信号监听线程产生的ECS事件
    signal: ProcessSignal--信号，公开
    impl Event for ProcessSignalReceived
        Event：公开trait实现

SignalListenerFailed：信号监听失败，公开结构体--STARTUP阶段无法解析信号、注册监听器或创建线程时产生的ECS事件
    message: String--错误信息，公开
    impl Event for SignalListenerFailed
        Event：公开trait实现

SignalOptions：信号配置，公开结构体
    signals: Vec<ProcessSignal>--信号，crate公开，至少包含一个值且不重复
    new() -> Self
        构造配置：公开关联函数
        行为：返回默认SignalOptions
    with_signals(mut self, signals: impl IntoIterator<Item = ProcessSignal>) -> Self
        设置信号：公开方法，signals提供需要监听的进程信号
        行为：按输入顺序保存并忽略重复值；结果为空时终止并报告signal list cannot be empty；返回自身
    signals(&self) -> &[ProcessSignal]
        获取信号：公开方法
        行为：返回去重后的信号只读切片
    impl Default for SignalOptions
        Default：公开trait实现
        default() -> Self
            构造默认配置：监听Interrupt与Terminate
            行为：返回包含两个默认信号的SignalOptions

SignalHandle：信号句柄，公开结构体--查询或提前停止当前App信号监听线程的共享入口
    inner: Arc<SignalInner>--内部状态，私有
    new() -> Self
        构造句柄：crate公开关联函数
        行为：构造iterator与thread均为空的共享SignalInner
    is_running(&self) -> bool
        检查运行状态：公开方法
        行为：监听线程存在且尚未结束时返回true
    shutdown(&self)
        停止监听：公开方法
        行为：调用SignalInner::shutdown；重复调用直接返回
    start(&self, configured: &[ProcessSignal], sender: RuntimeEventSender) -> io::Result<()>
        启动监听：crate公开方法，configured提供信号配置，sender将捕获结果发送进Runtime
        行为：
            锁定thread槽位，已经存在时返回Ok
            调用resolve_signals解析原始信号映射
            使用全部原始信号创建Signals与关闭Handle
            创建名为mecs-signal-listener的线程并持续阻塞读取原始信号
            原始信号存在于映射时发送ProcessSignalReceived
            保存关闭Handle与JoinHandle并返回Ok
    impl Resource for SignalHandle
        Resource：公开trait实现

SignalPlugin：信号插件，公开结构体
    options: SignalOptions--配置，私有
    new() -> Self
        构造插件：公开关联函数
        行为：返回默认SignalPlugin
    with_options(options: SignalOptions) -> Self
        使用配置构造：公开关联函数，options提供完整信号配置
        行为：保存options并返回SignalPlugin
    impl Default for SignalPlugin
        Default：公开trait实现
        default() -> Self
            构造默认插件：使用默认SignalOptions
            行为：返回默认SignalPlugin
    impl Plugin for SignalPlugin
        Plugin：公开trait实现
        build(self, app: &mut App)
            构建插件：向app安装SignalHandle和STARTUP System
            行为：
                RuntimeHandle不存在时终止并报告RuntimePlugin must be installed before SignalPlugin
                SignalHandle已经存在时终止并报告SignalPlugin can only be installed once
                创建SignalHandle并插入World
                将启动监听闭包挂到RuntimePlugin::STARTUP
                启动失败时发送SignalListenerFailed并唤醒Runtime
```

私有：
```text
SignalInner：信号内部状态，私有结构体--所有SignalHandle克隆共享的RAII状态
    iterator: Mutex<Option<Handle>>--signal-hook迭代器关闭句柄
    thread: Mutex<Option<JoinHandle<()>>>--监听线程句柄
    shutdown(&self)
        停止监听：私有方法
        行为：取出Handle并调用close；取出JoinHandle并join；线程panic时通过tracing记录error
    impl Drop for SignalInner
        Drop：私有trait实现
        drop(&mut self)
            自动回收：最后一个共享引用释放时关闭迭代器并join线程
            行为：忽略监听线程panic结果
```

## 函数

私有：
```text
resolve_signals(configured: &[ProcessSignal]) -> io::Result<HashMap<i32, ProcessSignal>>
    解析信号：私有函数，configured提供稳定进程信号
    行为：逐个调用raw_signal；解析失败时返回错误；相同原始信号只保留第一次映射

raw_signal(signal: ProcessSignal) -> io::Result<i32>
    解析Unix信号：Unix条件编译私有函数，signal是稳定信号语义
    行为：映射到当前平台原始信号；拒绝非正数、SIGKILL与SIGSTOP

raw_signal(signal: ProcessSignal) -> io::Result<i32>
    解析非Unix信号：非Unix条件编译私有函数，signal是稳定信号语义
    行为：Interrupt返回SIGINT，Terminate返回SIGTERM，其他变体返回Unsupported
```

## 逻辑

```text
构建：
    app.add_plugin(RuntimePlugin)
        -> app.add_plugin(SignalPlugin)
        -> SignalPlugin检查依赖与重复安装
        -> 插入SignalHandle并添加STARTUP System

启动：
    第一次tick
        -> STARTUP执行启动监听闭包
        -> resolve_signals解析配置
        -> signal-hook注册操作系统信号
        -> 创建mecs-signal-listener线程
    启动失败
        -> 发送SignalListenerFailed
        -> 唤醒Runtime
        -> 不自动重试或关闭Runtime

接收信号：
    操作系统发送已配置信号
        -> signal-hook唤醒监听线程
        -> 原始信号映射为ProcessSignal
        -> RuntimeEventSender发送ProcessSignalReceived
        -> Runtime被唤醒
        -> 下一帧装填事件读取存储
        -> 用户System自行决定关闭、重载、暂停或忽略

提前停止：
    SignalHandle::shutdown
        -> 关闭signal-hook迭代器
        -> 监听线程退出
        -> join监听线程

自动回收：
    App及其Schedule被销毁
        -> World和STARTUP闭包持有的SignalHandle克隆被释放
        -> 最后一个Arc触发SignalInner::drop
        -> 关闭迭代器并join线程

边界：
    SignalPlugin只监听当前进程的操作系统信号
    SignalPlugin不监听键盘事件；Ctrl+C仅因为终端驱动通常将其转换为SIGINT
    SignalPlugin不决定信号对应的业务策略，也不直接关闭Runtime、Server或Workspace
    SignalPlugin不缓冲、不限流信号事件；每次捕获都直接进入Core事件队列并唤醒Runtime
    SIGKILL与SIGSTOP无法被进程捕获，因此始终拒绝注册
    非Unix平台当前只保证Interrupt与Terminate
```

## 持有关系

```text
App
├── World
│   ├── RuntimeHandle Resource
│   └── SignalHandle Resource
│       └── Arc<SignalInner>
│           ├── Mutex<Option<Handle>>
│           └── Mutex<Option<JoinHandle<()>>>
└── RuntimePlugin::STARTUP
    └── 启动信号监听闭包
        ├── SignalHandle克隆
        └── Vec<ProcessSignal>

信号监听线程
├── Signals迭代器
├── HashMap<i32, ProcessSignal>
└── RuntimeEventSender
    ├── EventEmitter
    └── RuntimeHandle
```
