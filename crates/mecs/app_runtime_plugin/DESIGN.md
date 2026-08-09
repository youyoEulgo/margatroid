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

# Runtime

## 类型

公开：
```text
RuntimeMode：运行模式，公开枚举
    FixedFrame--固定帧
    EventDriven--事件驱动

RuntimeState：运行状态，公开枚举
    Working--工作
    Waiting--等待
    Sleeping--休眠
    Closed--关闭，暂时占位，触发条件以后再定义

RuntimeError：Runtime错误，公开枚举--描述Runtime公开API能够识别的配置错误与运行错误
    InvalidFrameRate {
        frame_rate: u64--帧率
    }
    RuntimePluginMissing
    RuntimePluginAlreadyInstalled
    RuntimeAlreadyRunning
    WakeChannelDisconnected
    GateOperationUnbalanced
    panic(self) -> !
        终止：crate公开方法，消费当前RuntimeError并使用Display文本终止执行
        行为：使用自身Display描述触发panic
    impl fmt::Display for RuntimeError
        Display：公开trait实现
        fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result
            格式化错误：formatter接收稳定的错误描述
            行为：输出包含错误类型与上下文的稳定错误描述
    impl std::error::Error for RuntimeError
        Error：公开trait实现

RuntimePlugin：Runtime插件，公开结构体
    mode: RuntimeMode--运行模式，私有
    frame_rate: Option<u64>--帧率，私有，仅FixedFrame模式使用
    STARTUP: &'static str--公开关联常量，等于"startup"，启动时执行一次
    PRE_UPDATE: &'static str--公开关联常量，等于"pre_update"，每帧第一个执行
    UPDATE: &'static str--公开关联常量，等于"update"，每帧第二个执行
    POST_UPDATE: &'static str--公开关联常量，等于"post_update"，每帧最后执行
    fixed(frame_rate: u64) -> Self
        构造固定帧Runtime：公开关联函数，frame_rate指定每秒执行帧数
        行为：frame_rate为0时终止并报告RuntimeError::InvalidFrameRate，否则构造固定帧模式
    impl Default for RuntimePlugin
        Default：公开trait实现
        default() -> Self
            构造默认Runtime：构造事件驱动模式
            行为：返回mode为EventDriven且frame_rate为None的RuntimePlugin
    impl Plugin for RuntimePlugin
        Plugin：公开trait实现
        build(self, app: &mut App)
            构建Runtime：向app安装默认Schedule及Runtime资源
            行为：
                RuntimeHandle或RuntimeControl已经存在时终止并报告RuntimeError::RuntimePluginAlreadyInstalled
                依次注册单次Schedule STARTUP和每帧Schedule PRE_UPDATE、UPDATE、POST_UPDATE
                创建容量为1的线程通知通道--多次唤醒合并为一个待处理通知
                使用通知发送端创建RuntimeHandle
                从World获取初始EventSnapshot
                使用配置、RuntimeHandle、通知接收端与快照创建RuntimeControl
                将RuntimeHandle与RuntimeControl作为Resource插入World

RuntimeHandle：Runtime句柄，公开结构体--可克隆并交给其他Plugin、System或异步线程
    blocker_count: Arc<AtomicUsize>--阻塞计数，私有，0表示开阀，非0表示仍有任务阻止下一帧开始
    wake_sender: SyncSender<()>--通知发送端，私有
    new(wake_sender: SyncSender<()>) -> Self
        构造Runtime句柄：crate公开关联函数，wake_sender发送运行时唤醒通知
        行为：使用wake_sender和初始值0构造共享blocker_count
    wake(&self)
        唤醒：公开方法，向Runtime发送一次非阻塞唤醒通知
        行为：通道已有通知时直接返回，通道断开时终止并报告RuntimeError::WakeChannelDisconnected
    open_gate(&self)
        开阀：公开方法，解除一层下一帧阻塞
        行为：原子检查blocker_count并在非0时减1；原值为0时报告GateOperationUnbalanced；变为0时调用wake
    close_gate(&self)
        关阀：公开方法，增加一层下一帧阻塞
        行为：blocker_count加1，当前帧继续完成，下一帧开始前进入等待
    is_gate_open(&self) -> bool
        检查阀：crate公开方法，查询是否不存在下一帧阻塞
        行为：blocker_count为0时返回true
    impl Resource for RuntimeHandle
        Resource：公开trait实现

RuntimeEventSender：Runtime事件发送器，公开结构体--可克隆并跨线程发送事件，同时唤醒Runtime
    emitter: EventEmitter--Core事件发射器，私有
    runtime: RuntimeHandle--Runtime句柄，私有
    new(emitter: EventEmitter, runtime: RuntimeHandle) -> Self
        构造发送器：crate公开关联函数，组合Core事件发射器与Runtime句柄
        行为：持有emitter与runtime
    send_event<E>(&self, event: E)
        发送事件：公开泛型方法，event是立即发送的事件
        约束：E: Event
        行为：调用EventEmitter::emit_event后调用RuntimeHandle::wake
    send_event_after<E>(&self, event: E, delay: u64)
        延迟发送事件：公开泛型方法，delay指定额外延迟帧数
        约束：E: Event
        行为：调用EventEmitter::emit_event_after后调用RuntimeHandle::wake

AppRunExt：App运行扩展，公开trait--由RuntimePlugin为App提供运行入口
    run(&mut self)
        运行：公开方法，由当前App进入Runtime循环
        行为：从World移出RuntimeControl；缺少Runtime时报告RuntimePluginMissing，已被移出时报告RuntimeAlreadyRunning；最后调用RuntimeControl::run
    impl AppRunExt for App
        AppRunExt for App：公开trait实现
        run(&mut self)
            运行：从当前App的World取得RuntimeControl
            行为：按trait定义执行

WorldEventExt：World事件扩展，公开trait--提供会唤醒Runtime的事件发送入口，不替换Core原有事件方法
    event_sender(&self) -> RuntimeEventSender
        获取Runtime事件发送器：公开方法
        行为：获取RuntimeHandle和World的EventEmitter并构造发送器；RuntimeHandle不存在时报告RuntimePluginMissing
    send_event<E>(&self, event: E)
        发送事件：公开泛型方法，event是立即发送的事件
        约束：E: Event
        行为：调用event_sender().send_event(event)
    send_event_after<E>(&self, event: E, delay: u64)
        延迟发送事件：公开泛型方法，delay指定额外延迟帧数
        约束：E: Event
        行为：调用event_sender().send_event_after(event, delay)
    impl WorldEventExt for World
        WorldEventExt for World：公开trait实现
        event_sender(&self) -> RuntimeEventSender
            获取Runtime事件发送器：使用World资源和Core事件发射器构造发送器
            行为：按trait定义执行
        send_event<E>(&self, event: E)
            发送事件：event是立即发送的事件
            约束：E: Event
            行为：按trait定义执行
        send_event_after<E>(&self, event: E, delay: u64)
            延迟发送事件：delay指定额外延迟帧数
            约束：E: Event
            行为：按trait定义执行
```

crate公开：
```text
RuntimeControl：Runtime控制器，crate公开结构体--配置阶段由World暂存，运行期间由AppRunExt::run移出并独占持有
    mode: RuntimeMode--运行模式
    frame_rate: Option<u64>--帧率，仅FixedFrame模式使用
    handle: RuntimeHandle--Runtime句柄
    wake_receiver: Mutex<Option<Receiver<()>>>--通知接收端，运行开始时取出
    event_snapshot: EventSnapshot--事件快照，每次判断下一帧前重新同步
    new(mode: RuntimeMode, frame_rate: Option<u64>, handle: RuntimeHandle, wake_receiver: Receiver<()>, event_snapshot: EventSnapshot) -> Self
        构造控制器：crate公开关联函数，接收运行配置、通知端与初始事件快照
        行为：使用全部参数构造RuntimeControl
    sync_event_snapshot(&mut self, app: &App)
        同步事件快照：私有方法，app提供最新World状态
        行为：调用World::event_snapshot覆盖event_snapshot
    status(&self) -> RuntimeState
        获取运行状态：私有方法
        行为：
            FixedFrame且开阀 -> Working，FixedFrame且关阀 -> Waiting
            EventDriven且关阀 -> Waiting
            EventDriven、开阀且有正常事件 -> Working
            EventDriven、开阀且只有pending事件 -> Waiting
            EventDriven、开阀且没有事件 -> Sleeping
            Closed暂不参与判断
    wait(wake_receiver: &Receiver<()>)
        等待唤醒：私有关联函数，wake_receiver接收唤醒通知
        行为：阻塞读取已从Resource取出的接收端；已有通知时立即返回，断开时报告WakeChannelDisconnected
    run(&mut self, app: &mut App)
        运行：crate公开方法，app是被当前控制器独占驱动的应用
        行为：短暂锁定wake_receiver并取出所有权，释放锁后按mode进入对应循环
    run_fixed_frame(&mut self, app: &mut App, wake_receiver: &Receiver<()>)
        固定帧运行：私有方法，按frame_rate驱动app
        行为：执行固定帧运行循环
    run_event_driven(&mut self, app: &mut App, wake_receiver: &Receiver<()>)
        事件驱动运行：私有方法，按事件快照驱动app
        行为：无条件执行初始帧后进入事件驱动运行循环
    run_initial_frame(&mut self, app: &mut App)
        执行初始帧：私有方法，在事件驱动循环开始前调用app.tick并同步EventSnapshot
        行为：保证空事件队列下也会执行STARTUP和第一帧循环Schedule
    impl Resource for RuntimeControl
        Resource：crate公开trait实现
```

错误边界：
```text
Runtime的配置错误与可识别运行错误使用RuntimeError统一描述
保持链式调用和运行控制方法遇到RuntimeError时终止并报告对应错误
锁中毒、原子计数溢出和Runtime内部状态不一致属于不变量破坏，直接panic且不进入RuntimeError
```

## 逻辑

```text
启动App：
    -> 链式调用add_plugin完成所有Plugin配置
    -> 挂载RuntimePlugin时，依次注册STARTUP、PRE_UPDATE、UPDATE、POST_UPDATE
    -> World暂存RuntimeControl并保留RuntimeHandle
    -> 调用AppRunExt::run
    -> 从World移出RuntimeControl，避免同时借用World内部Resource与整个App
    -> RuntimeControl独占驱动App并进入运行循环

固定帧运行循环：
    循环：
        同步EventSnapshot
        分流：RuntimeState
            Working：
                调用app.tick
                根据frame_rate等待
            Waiting或Sleeping：
                调用RuntimeControl::wait等待唤醒通知
            Closed：
                返回

事件驱动运行循环：
    调用run_initial_frame无条件执行一次app.tick
    循环：
        同步EventSnapshot
        分流：RuntimeState
            Working：
                调用app.fast_forward_tick
            Waiting或Sleeping：
                调用RuntimeControl::wait等待通知
            Closed：
                返回
```

## 持有关系

```text
配置阶段：
App
└── World
    ├── RuntimeControl Resource
    │   └── RuntimeHandle
    └── RuntimeHandle Resource

调用AppRunExt::run之后：
当前线程栈
└── RuntimeControl--从World移出所有权，独占驱动App
    └── RuntimeHandle

App
└── World
    └── RuntimeHandle Resource
```
