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

# SignalPlugin

设计原则：只将操作系统信号转换为ECS事件，不解释信号的业务含义，不决定Runtime是否关闭。

crate名称：signal_plugin。

## 类型

公开：
```text
进程信号：枚举--跨平台稳定的进程信号语义
    Interrupt
    Terminate
    Hangup
    Quit
    WindowChanged
    User1
    User2
    Raw(32位有符号整数)--仅Unix公开
    Clone：公开trait实现
    Copy：公开trait实现
    Debug：公开trait实现
    PartialEq：公开trait实现
    Eq：公开trait实现
    Hash：公开trait实现

收到进程信号：结构体--信号监听线程产生的ECS事件
    信号：进程信号--公开
    Event：公开trait实现
    Clone：公开trait实现
    Copy：公开trait实现
    Debug：公开trait实现
    PartialEq：公开trait实现
    Eq：公开trait实现

信号监听失败：结构体--STARTUP阶段无法解析信号、注册监听器或创建线程时产生的ECS事件
    错误信息：字符串--公开
    Event：公开trait实现
    Clone：公开trait实现
    Debug：公开trait实现
    PartialEq：公开trait实现
    Eq：公开trait实现

信号配置：结构体
    信号：数组<进程信号>--crate公开，至少包含一个值且不重复
    Clone：公开trait实现
    Debug：公开trait实现
    PartialEq：公开trait实现
    Eq：公开trait实现
    默认：公开trait实现
        签名：Default for 信号配置
        行为：监听Interrupt与Terminate
    构造：公开方法
        签名：new() -> Self
        行为：返回默认信号配置
    设置信号：公开泛型方法
        签名：with_signals(self, signals: impl IntoIterator<Item = 进程信号>) -> Self
        行为：
            清空原信号数组
            按输入顺序保存信号并忽略重复值
            结果为空时终止并报告signal list cannot be empty
            返回自身
    获取信号：公开方法
        签名：signals(&self) -> 数组<进程信号>引用
        行为：返回配置中去重后的信号只读切片

信号句柄：结构体--查询或提前停止当前App信号监听线程的共享入口
    内部状态：共享引用<信号内部状态>--私有
    Clone：公开trait实现
    Resource：公开trait实现
    是否正在运行：公开方法
        签名：is_running(&self) -> 布尔值
        行为：监听线程存在且尚未结束时返回true，否则返回false
    停止：公开方法
        签名：shutdown(&self)
        行为：关闭signal-hook迭代器并等待监听线程结束；重复调用直接返回
    构造：crate公开方法
        签名：new() -> Self
        行为：构造迭代器句柄与线程句柄均为空的共享内部状态
    启动：crate公开方法
        签名：start(&self, configured: 数组<进程信号>引用, sender: Runtime事件发送器) -> io::Result<()>
        行为：
            锁定线程槽位
            线程槽位已经存在时返回Ok
            调用resolve_signals将进程信号解析为原始信号映射
            使用全部原始信号创建signal-hook Signals与关闭句柄
            创建名为mecs-signal-listener的线程
            线程持续阻塞读取原始信号
            原始信号存在于映射时调用Runtime事件发送器::send_event发送收到进程信号
            将signal-hook关闭句柄保存进内部状态
            将线程句柄保存进线程槽位
            返回Ok

信号Plugin：结构体
    配置：信号配置--私有
    Clone：公开trait实现
    Debug：公开trait实现
    默认：公开trait实现
        签名：Default for 信号Plugin
        行为：使用默认信号配置构造Plugin
    构造：公开方法
        签名：new() -> Self
        行为：返回默认信号Plugin
    使用配置构造：公开方法
        签名：with_options(options: 信号配置) -> Self
        行为：保存options并返回信号Plugin
    Plugin：公开trait实现
        签名：Plugin for 信号Plugin
        行为：
            RuntimeHandle不存在时终止并报告RuntimePlugin must be installed before SignalPlugin
            信号句柄已经存在时终止并报告SignalPlugin can only be installed once
            创建信号句柄并将其作为Resource插入World
            注册收到进程信号与信号监听失败事件
            将启动信号监听System挂到RuntimePlugin::STARTUP
            STARTUP System调用信号句柄::start
            start失败时通过World::send_event发送信号监听失败事件并唤醒Runtime
```

私有：
```text
信号内部状态：结构体--所有信号句柄克隆共享的RAII状态
    迭代器句柄：互斥锁<可选<signal-hook Handle>>
    线程句柄：互斥锁<可选<JoinHandle<()>>>
    停止：私有方法
        签名：shutdown(&self)
        行为：
            取出signal-hook Handle并调用close，使Signals::forever结束
            取出线程句柄并join
            线程panic时通过tracing记录error
    Drop：私有trait实现
        签名：Drop for 信号内部状态
        行为：最后一个共享引用释放时关闭迭代器并join线程；忽略线程panic结果
```

## 函数

公开：
```text
无
```

私有：
```text
解析信号：函数
    签名：resolve_signals(configured: 数组<进程信号>引用) -> io::Result<HashMap<32位有符号整数, 进程信号>>
    行为：
        创建空原始信号映射
        逐个调用raw_signal解析配置中的进程信号
        解析失败时立即返回错误
        相同原始信号只保留第一次对应的进程信号
        返回原始信号映射

解析Unix信号：Unix条件编译函数
    签名：raw_signal(signal: 进程信号) -> io::Result<32位有符号整数>
    行为：
        将稳定信号变体映射到当前平台的SIGINT、SIGTERM、SIGHUP、SIGQUIT、SIGWINCH、SIGUSR1或SIGUSR2
        Raw直接使用调用者提供的原始值
        原始值小于等于0、等于SIGKILL或等于SIGSTOP时返回InvalidInput
        否则返回原始值

解析非Unix信号：非Unix条件编译函数
    签名：raw_signal(signal: 进程信号) -> io::Result<32位有符号整数>
    行为：Interrupt返回SIGINT，Terminate返回SIGTERM，其他变体返回Unsupported
```

## 持有关系

```text
App
├── World
│   ├── RuntimeHandle资源
│   ├── SignalHandle资源
│   │   └── 共享SignalInner
│   │       ├── signal-hook关闭句柄
│   │       └── 信号监听线程句柄
│   ├── ProcessSignalReceived事件存储
│   └── SignalListenerFailed事件存储
└── RuntimePlugin::STARTUP
    └── 启动信号监听System
        ├── SignalHandle克隆
        └── 配置后的进程信号数组

信号监听线程
├── signal-hook Signals迭代器
├── 原始信号到ProcessSignal的映射
└── RuntimeEventSender
    ├── Core EventEmitter
    └── RuntimeHandle
```

## 逻辑

```text
构建：
    app.add_plugin(RuntimePlugin)
        -> app.add_plugin(SignalPlugin)
        -> SignalPlugin检查依赖与重复安装
        -> 注册事件、SignalHandle资源与STARTUP System

启动：
    第一次tick
        -> RuntimePlugin::STARTUP执行启动信号监听System
        -> 解析配置中的进程信号
        -> signal-hook注册操作系统信号监听
        -> 创建mecs-signal-listener线程
    启动失败
        -> 发送SignalListenerFailed事件
        -> 唤醒Runtime
        -> SignalPlugin不自动重试、不关闭Runtime

接收信号：
    操作系统向进程发送已配置信号
        -> signal-hook唤醒监听线程
        -> 监听线程将原始信号映射为ProcessSignal
        -> RuntimeEventSender发送ProcessSignalReceived事件
        -> Runtime被唤醒
        -> 下一帧装填事件读取器
        -> 用户System读取事件并自行决定关闭、重载、暂停或忽略

提前停止：
    用户调用SignalHandle::shutdown
        -> 关闭signal-hook迭代器
        -> 监听线程退出
        -> join监听线程
        -> 后续已配置信号不再产生ECS事件

自动回收：
    App及其Schedule被销毁
        -> World中的SignalHandle与STARTUP System持有的SignalHandle克隆被释放
        -> 最后一个共享引用触发SignalInner::drop
        -> 关闭signal-hook迭代器并join监听线程

边界：
    SignalPlugin只监听当前进程的操作系统信号
    SignalPlugin不监听键盘事件；Ctrl+C仅因为终端驱动通常将其转换为SIGINT
    SignalPlugin不决定信号对应的业务策略，也不直接关闭Runtime、Server或Workspace
    SignalPlugin不缓冲、不限流信号事件；每次捕获都直接进入Core事件队列并唤醒Runtime
    SIGKILL与SIGSTOP无法被进程捕获，因此始终拒绝注册
    非Unix平台当前只保证Interrupt与Terminate
```
