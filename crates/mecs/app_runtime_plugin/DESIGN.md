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

RuntimePlugin：结构体
    运行模式：运行模式--私有
    帧率：可选无符号整数--私有，仅固定帧模式使用
    默认：公开trait实现
        签名：Default for RuntimePlugin
        行为：构造事件驱动模式的RuntimePlugin
    固定帧：公开方法
        签名：fixed(frame_rate: 无符号整数) -> Self
        行为：
            如果frame_rate为0，终止并报告帧率必须大于0
            使用用户指定的帧率构造固定帧模式的RuntimePlugin
    Plugin：公开trait实现
        签名：build(self, app: &mut App)
        行为：
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
            如果通知通道断开，终止并报告错误
    开阀：公开方法
        签名：open_gate(&self)
        行为：
            原子地检查阻塞计数并在非0时减1
            如果检查时阻塞计数为0，终止并报告开关阀调用不匹配
            如果阻塞计数变为0，调用wake
    关阀：公开方法
        签名：close_gate(&self)
        行为：将阻塞计数加1，当前帧继续完成，下一帧开始前进入等待

App运行拓展：trait--由RuntimePlugin为App提供
    运行：公开方法
        签名：run(&mut self)
        行为：
            从World移出RuntimeControl并取得所有权
            如果RuntimeControl不存在，终止并报告RuntimePlugin未挂载或App已经开始运行
            调用RuntimeControl::run并传入App可变引用

World事件拓展：trait--由RuntimePlugin为World提供，不替换core原有事件方法
    发送事件：公开泛型方法
        签名：emit_event<事件类型:Event>(&self, event: 事件类型)
        行为：
            获取RuntimeHandle
            如果RuntimeHandle不存在，终止并报告RuntimePlugin未挂载
            调用core事件队列的send_event
            调用RuntimeHandle::wake
    延迟发送事件：公开泛型方法
        签名：emit_event_after<事件类型:Event>(&self, event: 事件类型, delay: 无符号整数)
        行为：
            获取RuntimeHandle
            如果RuntimeHandle不存在，终止并报告RuntimePlugin未挂载
            调用core事件队列的send_event_after
            调用RuntimeHandle::wake
```

私有：
```text
RuntimeControl：结构体--配置阶段由World暂存，运行期间由App::run移出并独占持有
    运行模式：运行模式
    帧率：可选无符号整数
    RuntimeHandle：RuntimeHandle
    通知接收端：互斥锁<线程通知接收端>--包装互斥锁以满足Resource约束
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
        签名：wait(&self)
        行为：阻塞读取通知接收端，已有通知时立即返回，没有通知时等待
    运行：crate公开方法
        签名：run(&mut self, app: &mut App)
        行为：根据运行模式进入对应的运行循环
    固定帧运行：私有方法
        签名：run_fixed_frame(&mut self, app: &mut App)
        行为：按指定帧率执行固定帧运行循环
    事件驱动运行：私有方法
        签名：run_event_driven(&mut self, app: &mut App)
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
    -> 挂载RuntimePlugin时，World暂存RuntimeControl并保留RuntimeHandle
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
