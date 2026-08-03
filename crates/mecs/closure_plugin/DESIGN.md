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

# ClosurePlugin

闭包模式允许开发者把一次性同步闭包发送给指定Schedule，由显式挂载的ClosureSystem取得闭包并临时借用World执行。

## 类型

公开：
```text
ClosureError：闭包插件错误，公开枚举--描述闭包基础设施的配置错误与使用错误
    RuntimePluginMissing
    ClosurePluginMissing
    ClosurePluginAlreadyInstalled
    ClosureSystemAlreadyRegistered {
        schedule: String--Schedule名称
    }
    ClosureSystemNotRegistered {
        schedule: String--Schedule名称
    }
    panic(self) -> !
        终止：crate公开方法，消费当前ClosureError并使用Display文本终止执行
        行为：使用自身Display描述触发panic
    impl fmt::Display for ClosureError
        Display：公开trait实现
        fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result
            格式化错误：formatter接收稳定的错误描述
            行为：输出包含错误类型与上下文的稳定错误描述
    impl std::error::Error for ClosureError
        Error：公开trait实现

ClosurePlugin：闭包插件，公开单元结构体
    impl Plugin for ClosurePlugin
        Plugin：公开trait实现
        build(self, app: &mut App)
            构建插件：安装闭包注册表，不自动挂载ClosureSystem
            行为：
                RuntimeHandle不存在时报告ClosureError::RuntimePluginMissing
                ClosureRegistry已经存在时报告ClosureError::ClosurePluginAlreadyInstalled
                创建ClosureRegistry并作为Resource插入World

AppClosureExt：App闭包扩展，公开trait--显式挂载指定Schedule的闭包处理System
    add_closure_system(&mut self, schedule: &str) -> &mut Self
        添加闭包System：公开方法，schedule指定允许执行一次性闭包的阶段
        行为：
            ClosurePlugin未安装时报告ClosureError::ClosurePluginMissing
            schedule已经注册时报告ClosureError::ClosureSystemAlreadyRegistered
            在ClosureRegistry中注册schedule
            向schedule添加ClosureSystem
            返回App可变引用
    impl AppClosureExt for App
        AppClosureExt for App：公开trait实现
        add_closure_system(&mut self, schedule: &str) -> &mut Self
            添加闭包System：按trait定义注册阶段并挂载ClosureSystem

WorldClosureExt：World闭包扩展，公开trait--向指定Schedule发送一次性同步闭包
    send_closure<Closure>(&self, schedule: &str, closure: Closure)
        发送闭包：公开泛型方法，schedule指定执行阶段，closure在该阶段临时取得&mut World
        约束：Closure: FnOnce(&mut World) + Send + 'static
        行为：
            ClosurePlugin未安装时报告ClosureError::ClosurePluginMissing
            schedule未注册时报告ClosureError::ClosureSystemNotRegistered
            擦除closure并构造ClosureRequest
            调用WorldEventExt::send_event发送ClosureRequest，由Runtime统一完成事件入队与唤醒
    impl WorldClosureExt for World
        WorldClosureExt for World：公开trait实现
        send_closure<Closure>(&self, schedule: &str, closure: Closure)
            发送闭包：按trait定义包装闭包并复用Runtime事件发送入口
            约束：Closure: FnOnce(&mut World) + Send + 'static
```

私有：
```text
ErasedClosure：擦除闭包，私有类型别名--等于Box<dyn FnOnce(&mut World) + Send + 'static>

ClosureRegistry：闭包注册表，私有结构体--记录已经显式挂载ClosureSystem的Schedule
    schedules: HashSet<String>--已注册的Schedule名称
    new() -> Self
        构造注册表：私有关联函数，构造空集合
    register(&mut self, schedule: String) -> bool
        注册阶段：私有方法，首次插入schedule时返回true
    contains(&self, schedule: &str) -> bool
        查询阶段：私有方法，schedule已经注册时返回true
    impl Resource for ClosureRegistry
        Resource：私有trait实现

ClosureRequest：闭包请求，私有结构体--通过单一事件类型传递目标Schedule和任意一次性同步闭包
    target_schedule: String--目标Schedule
    closure: Mutex<Option<ErasedClosure>>--只能取出一次的擦除闭包
    new(target_schedule: String, closure: ErasedClosure) -> Self
        构造请求：私有关联函数，保存目标Schedule和擦除闭包
    take_for(&self, schedule: &str) -> Option<ErasedClosure>
        按阶段取出：私有方法，目标不匹配时返回None，匹配时第一次调用转移闭包，后续返回None
    impl Event for ClosureRequest
        Event：私有trait实现

ClosureSystem：闭包System，私有结构体--只取得并执行目标为当前Schedule的一次性同步闭包
    schedule: String--当前System所属Schedule
    new(schedule: String) -> Self
        构造System：私有关联函数，保存schedule
    impl System for ClosureSystem
        System：私有trait实现
        run(&mut self, world: &mut World)
            执行闭包：读取ClosureRequest并依次执行属于当前Schedule的闭包
            行为：
                创建局部closures数组
                在独立作用域中借用world的ClosureRequest读取器
                对每个请求调用take_for，将成功取得的闭包移入closures
                结束读取器作用域，释放对world的共享借用
                依次调用closures中的闭包并为每个闭包临时传入&mut World
```

## 持有关系

```text
App
├── Schedule
│   └── ClosureSystem--开发者按Schedule显式挂载
│       └── schedule: String
└── World
    ├── RuntimeHandle Resource
    └── ClosureRegistry Resource
        └── schedules

ClosureRequest
├── target_schedule
└── Mutex<Option<ErasedClosure>>
```

## 逻辑

```text
启动闭包基础设施：
    app.add_plugin(RuntimePlugin)
        -> app.add_plugin(ClosurePlugin)
        -> 插入ClosureRegistry
        -> 不自动挂载ClosureSystem

挂载闭包System：
    app.add_closure_system(schedule)
        -> ClosureRegistry检查schedule未注册
        -> 创建ClosureSystem { schedule }
        -> 挂载到schedule

发送同步闭包：
    world.send_closure(schedule, closure)
        -> ClosureRegistry确认schedule已挂载ClosureSystem
        -> 构造ClosureRequest { target_schedule: schedule, closure }
        -> 调用WorldEventExt::send_event
            -> Core事件入队
            -> RuntimeHandle::wake
    目标ClosureSystem读取ClosureRequest
        -> 在读取器作用域内取出属于当前schedule的闭包
        -> 结束读取器对World的共享借用
        -> 逐个执行closure(&mut World)

边界：
    ClosurePlugin不直接调用EventEmitter，不复制Runtime的唤醒逻辑
    ClosurePlugin只负责把闭包转换为ClosureRequest，事件发送与唤醒始终由RuntimePlugin负责
    ClosureSystem只执行同步闭包，不认识Future、AsyncContext、pending事件或Runtime阀
    其他Plugin可以把自身操作包装成同步闭包，再通过send_closure复用同一套调度能力
    send_closure只能选择已挂载ClosureSystem的Schedule，绝不在运行时修改Schedule
    请求在发送后的下一次tick进入读取存储，再由目标ClosureSystem处理
    锁中毒属于不变量破坏，直接panic；已经被取得的闭包不会再次执行
```

## 职责

```text
ClosurePlugin：
    提供一次性同步闭包的事件包装与Schedule路由
    提供显式挂载ClosureSystem的API
    复用RuntimePlugin的事件发送与唤醒
    不提供异步执行器
    不持有Runtime循环

ClosureSystem：
    结束事件读取借用后，临时把&mut World交给闭包
    不让闭包跨越本次System执行持有World

RuntimePlugin：
    只负责事件入队后的Runtime唤醒
    不解释ClosureRequest，也不执行闭包
```
