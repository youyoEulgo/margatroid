# Runtime

## 类型

公开：
```text
运行模式：枚举
    固定帧
    事件驱动

运行状态：枚举
    工作
    等待
    休眠
    关闭--暂时占位，触发条件以后再定义

RuntimeError：枚举--描述Runtime公开API能够识别的配置错误与运行错误
    NonExhaustive：公开属性--允许后续版本增加错误变体
    InvalidFrameRate { frame_rate: 无符号整数 }
    RuntimePluginMissing
    RuntimePluginAlreadyInstalled
    RuntimeAlreadyRunning
    WakeChannelDisconnected
    GateOperationUnbalanced
    终止：crate公开方法
        签名：panic(self) -> Never
        行为：使用自身Display描述触发panic
    Debug：公开trait实现
        签名：Debug for RuntimeError
    Display：公开trait实现
        签名：Display for RuntimeError
        行为：输出包含错误类型与上下文的稳定错误描述
    Error：公开trait实现
        签名：Error for RuntimeError

RuntimePlugin：结构体
    运行模式：运行模式--私有
    帧率：可选无符号整数--私有，仅固定帧模式使用
    STARTUP: 公开关联字符串常量 = "startup"--启动时执行一次
    PRE_UPDATE: 公开关联字符串常量 = "pre_update"--每帧第一个执行
    UPDATE: 公开关联字符串常量 = "update"--每帧第二个执行
    POST_UPDATE: 公开关联字符串常量 = "post_update"--每帧最后执行
    默认：公开trait实现
        签名：Default for RuntimePlugin
        行为：构造事件驱动模式的RuntimePlugin
    固定帧：公开方法
        签名：fixed(frame_rate: 无符号整数) -> Self
        行为：
            如果frame_rate为0，终止并报告RuntimeError::InvalidFrameRate
            使用用户指定的帧率构造固定帧模式的RuntimePlugin
    Plugin：公开trait实现
        签名：build(self, app: &mut App)
        行为：
            如果RuntimeHandle或RuntimeControl已经存在，终止并报告RuntimeError::RuntimePluginAlreadyInstalled
            依次注册单次Schedule RuntimePlugin::STARTUP和每帧Schedule RuntimePlugin::PRE_UPDATE、RuntimePlugin::UPDATE、RuntimePlugin::POST_UPDATE
            创建容量为1的线程通知通道--多次唤醒合并为一个待处理通知
            使用通知发送端与初始为0的阻塞计数创建RuntimeHandle
            从World获取初始事件快照
            使用运行模式、帧率、RuntimeHandle克隆、通知接收端与初始事件快照创建RuntimeControl
            将RuntimeHandle与RuntimeControl作为Resource插入World

RuntimeHandle：结构体--可以克隆并交给其他Plugin、System或异步线程
    阻塞计数：共享原子无符号整数--私有，0表示开阀，非0表示仍有任务阻止下一帧开始
    通知发送端：线程通知发送端--私有
    Resource：公开trait实现
    Clone：公开trait实现
    唤醒：公开方法
        签名：wake(&self)
        行为：
            尝试向通知通道非阻塞发送一次唤醒通知
            如果通道已有通知，直接返回
            如果通知通道断开，终止并报告RuntimeError::WakeChannelDisconnected
    开阀：公开方法
        签名：open_gate(&self)
        行为：
            原子地检查阻塞计数并在非0时减1
            如果检查时阻塞计数为0，终止并报告RuntimeError::GateOperationUnbalanced
            如果阻塞计数变为0，调用wake
    关阀：公开方法
        签名：close_gate(&self)
        行为：将阻塞计数加1，当前帧继续完成，下一帧开始前进入等待

Runtime事件发送器：结构体--可克隆并跨线程发送事件，同时唤醒Runtime
    Core事件发射器：事件发射器--私有
    Runtime句柄：RuntimeHandle--私有
    Clone：公开trait实现
    发送事件：公开泛型方法
        签名：send_event<事件类型:Event>(&self, event: 事件类型)
        行为：调用Core事件发射器::emit_event后调用RuntimeHandle::wake
    延迟发送事件：公开泛型方法
        签名：send_event_after<事件类型:Event>(&self, event: 事件类型, delay: 无符号整数)
        行为：调用Core事件发射器::emit_event_after后调用RuntimeHandle::wake

App运行拓展：trait--由RuntimePlugin为App提供
    运行：公开方法
        签名：run(&mut self)
        行为：
            从World移出RuntimeControl并取得所有权
            如果RuntimeControl不存在且RuntimeHandle不存在，终止并报告RuntimeError::RuntimePluginMissing
            如果RuntimeControl不存在且RuntimeHandle存在，终止并报告RuntimeError::RuntimeAlreadyRunning
            调用RuntimeControl::run并传入App可变引用

World事件拓展：trait--由RuntimePlugin为World提供，不替换core原有事件方法
    获取Runtime事件发送器：公开方法
        签名：event_sender(&self) -> Runtime事件发送器
        行为：获取RuntimeHandle和World事件发射器并构造Runtime事件发送器，RuntimeHandle不存在时终止并报告RuntimeError::RuntimePluginMissing
    发送事件：公开泛型方法
        签名：send_event<事件类型:Event>(&self, event: 事件类型)
        行为：
            调用event_sender取得Runtime事件发送器
            调用Runtime事件发送器::send_event
    延迟发送事件：公开泛型方法
        签名：send_event_after<事件类型:Event>(&self, event: 事件类型, delay: 无符号整数)
        行为：
            调用event_sender取得Runtime事件发送器
            调用Runtime事件发送器::send_event_after
```

错误边界：
```text
Runtime的配置错误与可识别运行错误使用RuntimeError统一描述
保持链式调用和运行控制方法遇到RuntimeError时终止并报告对应错误
锁中毒、原子计数溢出和Runtime内部状态不一致属于不变量破坏，直接panic且不进入RuntimeError
```

私有：
```text
RuntimeControl：结构体--配置阶段由World暂存，运行期间由App::run移出并独占持有
    运行模式：运行模式
    帧率：可选无符号整数
    RuntimeHandle：RuntimeHandle
    通知接收端：互斥锁<可选线程通知接收端>--包装互斥锁以满足Resource约束，运行开始时取出
    事件快照：事件快照--每次准备判断下一帧前重新同步
    Resource：crate公开trait实现

RuntimeControl：实现
    构造：crate公开方法
        签名：new(运行模式, 帧率, RuntimeHandle, 通知接收端, 事件快照) -> Self
        行为：使用全部参数构造RuntimeControl
    同步事件快照：私有方法
        签名：sync_event_snapshot(&mut self, app: &App)
        行为：调用World::event_snapshot取得新的事件快照并覆盖自己的旧快照
    状态：私有方法
        签名：status(&self) -> 运行状态
        行为：
            分流：运行模式
                固定帧：
                    阻塞计数为0 -> 工作
                    阻塞计数非0 -> 等待
                事件驱动：
                    阻塞计数非0 -> 等待
                    阻塞计数为0且有正常事件 -> 工作
                    阻塞计数为0且只有pending事件 -> 等待
                    阻塞计数为0且没有任何事件 -> 休眠
            关闭状态暂时不参与判断
    等待唤醒：私有方法
        签名：wait(通知接收端引用)
        行为：直接阻塞读取已经从Resource取出的通知接收端，已有通知时立即返回，没有通知时等待，不持有互斥锁
    运行：crate公开方法
        签名：run(&mut self, app: &mut App)
        行为：短暂锁定通知接收端并取出所有权，释放锁后根据运行模式进入对应的运行循环
    固定帧运行：私有方法
        签名：run_fixed_frame(&mut self, app: &mut App, 通知接收端引用)
        行为：按指定帧率执行固定帧运行循环
    事件驱动运行：私有方法
        签名：run_event_driven(&mut self, app: &mut App, 通知接收端引用)
        行为：执行事件驱动运行循环
```

## 持有关系

```text
配置阶段：
App
└── World
    ├── RuntimeControl资源
    │   └── RuntimeHandle
    └── RuntimeHandle资源

调用App::run之后：
当前线程栈
└── RuntimeControl--从World移出所有权，独占驱动App
    └── RuntimeHandle

App
└── World
    └── RuntimeHandle资源
```

## 逻辑

```text
启动App：
    -> 链式调用add_plugin完成所有Plugin配置
    -> 挂载RuntimePlugin时，依次注册RuntimePlugin::STARTUP、RuntimePlugin::PRE_UPDATE、RuntimePlugin::UPDATE、RuntimePlugin::POST_UPDATE
    -> World暂存RuntimeControl并保留RuntimeHandle
    -> 调用App运行拓展的run
    -> 从World移出RuntimeControl，避免同时借用World内部资源与整个App
    -> RuntimeControl独占驱动App并进入运行循环

固定帧运行循环：
    循环：
        同步事件快照
        分流：运行状态
            工作：
                调用app.tick
                根据用户指定的帧率等待
            等待或休眠：
                调用wait等待唤醒通知
            关闭：
                返回

事件驱动运行循环：
    循环：
        同步事件快照
        分流：运行状态
            工作：
                调用app.fast_forward_tick
            等待或休眠：
                调用wait等待通知
            关闭：
                返回
```
