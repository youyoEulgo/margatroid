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

# Error
## 类型
公开：
```text
CoreError：枚举--描述core公开API能够识别的配置错误与状态错误
    NonExhaustive：公开属性--允许后续版本增加错误变体
    AppAlreadyStarted
    ScheduleAlreadyExists { name: 字符串 }
    ScheduleNotFound { name: 字符串 }
    EntityCapacityExhausted
    PendingEventAlreadyCompleted
    终止：crate公开方法
        签名：panic(self) -> Never
        行为：使用自身Display描述触发panic
    Debug：公开trait实现
        签名：Debug for CoreError
    Display：公开trait实现
        签名：Display for CoreError
        行为：输出包含错误类型与上下文的稳定错误描述
    Error：公开trait实现
        签名：Error for CoreError
```

## 逻辑
```text
错误边界：
    可由调用者输入或当前公开状态判断的错误，使用CoreError统一描述
    保持链式调用的配置方法遇到CoreError时终止并报告对应错误
    锁中毒、类型擦除不一致和内部计数不一致属于core不变量破坏，直接panic且不进入CoreError
```

# Component
## 类型
私有：
```text
擦除稀疏集合列：trait--擦除组件列的具体组件类型，供组件注册表统一持有
    继承：Any + Send + Sync + 'static
    移除实体：私有方法
        签名：remove_entity(实体：Entity)
        行为：移除并丢弃该Entity在当前列中的组件，不存在时不执行操作
    转换为Any只读引用：私有方法
        签名：as_any() -> Any只读引用
        行为：返回当前组件列的Any只读引用
    转换为Any可变引用：私有方法
        签名：as_any_mut() -> Any可变引用
        行为：返回当前组件列的Any可变引用
```

公开：
```text
组件：trait--实体可持有的数据类型
    继承：Send + Sync + 'static
    实现：由开发者为实体数据类型显式实现
```

crate公开：
```text
稀疏集合列<组件类型:组件>：结构体--连续存储单一具体组件类型，通过Entity索引定位稠密数据
    稀疏索引：数组<可选<无符号整数>>--Entity索引到稠密位置的映射
    实体：数组<Entity>--与稠密组件一一对应
    组件：数组<组件类型>--同一具体组件类型的连续稠密数据
    构造：crate公开方法
        签名：new() -> Self
        行为：构造三个数组均为空的稀疏集合列
    定位稠密位置：私有方法
        签名：dense_index(实体：Entity) -> 可选<无符号整数>
        行为：
            使用Entity索引查找稀疏索引中的稠密位置
            如果位置不存在，返回空
            比较稠密位置中Entity的代数
            如果代数相同，返回稠密位置，否则返回空
    插入组件：crate公开方法
        签名：insert(实体：Entity, 组件：组件类型)
        行为：
            如果dense_index返回稠密位置，替换该位置的组件并返回
            扩展稀疏索引以容纳Entity索引
            将新稠密位置写入稀疏索引
            将Entity和组件追加到对应稠密数组
    读取组件：crate公开方法
        签名：get(实体：Entity) -> 可选<组件类型只读引用>
        行为：返回dense_index定位的组件只读引用，无法定位时返回空
    可变读取组件：crate公开方法
        签名：get_mut(实体：Entity) -> 可选<组件类型可变引用>
        行为：返回dense_index定位的组件可变引用，无法定位时返回空
    迭代组件：crate公开方法
        签名：iter() -> 迭代器<Item = (Entity只读引用, 组件类型只读引用)>
        行为：按稠密数组顺序成对迭代Entity和组件的只读引用
    移除组件：crate公开方法
        签名：remove(实体：Entity) -> 可选<组件类型>
        行为：
            如果dense_index返回空，返回空，否则记录待移除稠密位置
            清空待移除Entity的稀疏索引
            分别对实体数组和组件数组的相同位置执行swap_remove
            如果原末尾Entity被移动到待移除位置，更新该Entity在稀疏索引中的稠密位置
            返回被移除组件
    擦除存储：私有trait实现
        签名：擦除稀疏集合列 for 稀疏集合列<组件类型>
        实现：
            remove_entity(实体：Entity)--调用remove并丢弃返回的组件
            as_any() -> Any只读引用--返回自身的Any只读引用
            as_any_mut() -> Any可变引用--返回自身的Any可变引用

组件注册表：结构体--管理所有组件类型对应的稀疏集合列，不负责检查Entity是否存活
    组件列：类型映射<TypeId, 类型擦除<擦除稀疏集合列>>
    构造：crate公开方法
        签名：new() -> Self
        行为：创建不包含任何组件列的注册表
    插入组件：crate公开泛型方法
        签名：insert<组件类型:组件>(实体：Entity, 组件：组件类型)
        行为：
            获取组件类型的TypeId
            获取对应的擦除稀疏集合列，不存在时创建并擦除稀疏集合列<组件类型>
            将擦除稀疏集合列恢复为稀疏集合列<组件类型>可变引用
            调用稀疏集合列<组件类型>::insert
    读取组件：crate公开泛型方法
        签名：get<组件类型:组件>(实体：Entity) -> 可选<组件类型只读引用>
        行为：
            获取对应的擦除稀疏集合列
            将其恢复为稀疏集合列<组件类型>只读引用
            返回稀疏集合列<组件类型>::get的结果
            如果组件列不存在，返回空
    可变读取组件：crate公开泛型方法
        签名：get_mut<组件类型:组件>(实体：Entity) -> 可选<组件类型可变引用>
        行为：
            获取对应的擦除稀疏集合列
            将其恢复为稀疏集合列<组件类型>可变引用
            返回稀疏集合列<组件类型>::get_mut的结果
            如果组件列不存在，返回空
    迭代组件：crate公开泛型方法
        签名：iter<组件类型:组件>() -> 迭代器<Item = (Entity只读引用, 组件类型只读引用)>
        行为：恢复对应的稀疏集合列<组件类型>并返回其只读迭代器，组件列不存在时返回空迭代器
    移除组件：crate公开泛型方法
        签名：remove<组件类型:组件>(实体：Entity) -> 可选<组件类型>
        行为：
            获取对应的擦除稀疏集合列
            将其恢复为稀疏集合列<组件类型>可变引用
            返回稀疏集合列<组件类型>::remove的结果
            如果组件列不存在，返回空
    检查组件：crate公开泛型方法
        签名：contains<组件类型:组件>(实体：Entity) -> 布尔值
        行为：对应稀疏集合列<组件类型>存在且能够定位该Entity时返回真，否则返回假
    移除实体的全部组件：crate公开方法
        签名：remove_entity(实体：Entity)
        行为：遍历所有擦除稀疏集合列并调用擦除稀疏集合列::remove_entity
```

# Entity
## 类型
私有：
```text
实体槽位：结构体--记录一个Entity索引的当前状态
    代数：32位无符号整数
    是否存活：布尔值
```

公开：
```text
Entity：结构体--带代数的实体标识，避免索引复用后旧标识指向新实体
    标识：64位无符号整数--私有，高32位为代数，低32位为索引
    构造：crate公开方法
        签名：new(索引：32位无符号整数, 代数：32位无符号整数) -> Self
        行为：将代数和索引组合为实体标识
    获取索引：公开方法
        签名：index() -> 32位无符号整数
        行为：返回实体标识的低32位
    获取代数：公开方法
        签名：generation() -> 32位无符号整数
        行为：返回实体标识的高32位
    标准特征：公开trait实现
        签名：Copy + Clone + PartialEq + Eq + Hash + Debug for Entity
        行为：支持复制、比较、哈希和调试输出
```

crate公开：
```text
实体分配器：结构体--分配和回收Entity标识
    槽位：数组<实体槽位>--Entity索引对应的代数和存活状态
    空闲索引：栈<32位无符号整数>--可以复用的Entity索引
    存活数量：无符号整数
    构造：crate公开方法
        签名：new() -> Self
        行为：构造槽位和空闲索引均为空、存活数量为0的实体分配器
    分配：crate公开方法
        签名：allocate() -> Entity
        行为：
            如果空闲索引非空，弹出索引并将对应槽位标记为存活
            否则，使用槽位数量作为新索引
                如果新索引无法表示为32位无符号整数，终止并报告CoreError::EntityCapacityExhausted
                追加代数为0且存活的槽位
            存活数量加1
            返回使用该索引和槽位当前代数构造的Entity
    回收：crate公开方法
        签名：release(实体：Entity) -> 布尔值
        行为：
            如果is_alive返回假，返回假
            将对应槽位标记为不存活
            存活数量减1
            如果代数可以加1，推进代数并将索引推入空闲索引
            如果代数已达上限，永久退役该索引
            返回真
    检查存活：crate公开方法
        签名：is_alive(实体：Entity) -> 布尔值
        行为：仅当索引存在、槽位存活且代数相同时返回真
    获取存活数量：crate公开方法
        签名：len() -> 无符号整数
        行为：返回存活数量
    迭代存活实体：crate公开方法
        签名：iter_alive() -> 迭代器<Item = Entity>
        行为：遍历存活的实体槽位，使用槽位索引和当前代数构造并返回Entity

```

# Resource
## 类型
公开：
```text
Resource：trait--World持有的全局单例数据类型
    继承：Send + Sync + 'static
    实现：由开发者为World全局单例类型显式实现
```

crate公开：
```text
资源注册表：结构体--按资源类型持有全局单例
    资源：类型映射<TypeId, 类型擦除<Any + Send + Sync>>
    构造：crate公开方法
        签名：new() -> Self
        行为：创建不包含任何资源的注册表
    插入资源：crate公开泛型方法
        签名：insert<资源类型:Resource>(资源：资源类型)
        行为：擦除资源具体类型并按TypeId插入；已有同类型资源时替换旧资源
    读取资源：crate公开泛型方法
        签名：get<资源类型:Resource>() -> 可选<资源类型只读引用>
        行为：获取对应的类型擦除资源，恢复为资源类型只读引用并返回，不存在时返回空
    可变读取资源：crate公开泛型方法
        签名：get_mut<资源类型:Resource>() -> 可选<资源类型可变引用>
        行为：获取对应的类型擦除资源，恢复为资源类型可变引用并返回，不存在时返回空
    移除资源：crate公开泛型方法
        签名：remove<资源类型:Resource>() -> 可选<资源类型>
        行为：移除对应的类型擦除资源，恢复为具体资源类型并返回，不存在时返回空
    检查资源：crate公开泛型方法
        签名：contains<资源类型:Resource>() -> 布尔值
        行为：对应资源类型的TypeId存在时返回真，否则返回假
```

# Event
## 类型
私有：
```text
事件状态：枚举
    pending
    正常：
        事件体：类型擦除<擦除事件>
        剩余延迟帧：64位无符号整数--事件在执行前还需等待多少次tick

事件节点：类型别名<共享引用<互斥锁<事件状态>>>--事件队列与异步任务可以安全持有并修改同一事件

擦除事件：trait--跨线程排队时擦除具体类型，到期后在主线程恢复对应存储操作
    装填读取存储：私有方法
        签名：push_into(self: 类型擦除<Self>, 注册表：事件读取存储注册表可变引用)
        行为：由具体事件类型实现，调用注册表::push转移事件所有权
    事件类型实现：私有泛型trait实现
        签名：擦除事件 for 事件类型:事件特征
        行为：恢复事件所有权并调用事件读取存储注册表::push

事件读取存储<事件类型:事件特征>：结构体--持有单一类型的本次更新事件
    事件：数组<事件类型>
    装填事件：私有方法
        签名：push(事件体：事件类型)
        行为：取得事件体所有权并推入事件数组
    擦除存储：私有trait实现
        签名：擦除事件读取存储 for 事件读取存储<事件类型>
        实现：
            clear()--清空事件数组
            as_any() -> Any只读引用--返回自身的Any只读引用
            as_any_mut() -> Any可变引用--返回自身的Any可变引用

擦除事件读取存储：trait
    清空：私有方法
        签名：clear()
    转换为Any只读引用：私有方法
        签名：as_any() -> Any只读引用
    转换为Any可变引用：私有方法
        签名：as_any_mut() -> Any可变引用
```

crate公开
```text
事件读取存储注册表：结构体--由World持有
    存储：类型映射<类型擦除<擦除事件读取存储>>
    构造：crate公开方法
        签名：new() -> Self
        行为：创建不包含任何事件读取存储的注册表
    获取事件读取器：crate公开泛型方法
        签名：reader<事件类型:事件特征>() -> 事件读取器<'_, 事件类型>
        行为：
            类型已出现时返回持有对应事件切片的读取器
            类型从未出现时返回持有空切片的读取器
    装填事件：私有方法
        签名：push<事件类型:事件特征>(事件体：事件类型)
        行为：
            如果对应类型存储不存在，创建事件读取存储<事件类型>
            将存储恢复为事件读取存储<事件类型>
            调用事件读取存储::push转移事件所有权
    清空：crate公开方法
        签名：clear()
        行为：
            遍历所有事件读取存储
            调用事件读取存储::clear，保留已创建的存储
```

公开：
```text
Result<T, E>：Event trait实现
    约束：T与E满足Send + Sync + 'static
    行为：允许Result<T, E>作为事件发送和读取

事件快照：结构体--事件队列持有，对外只返回字段可读的克隆
    正常事件数：公开无符号整数
    pending事件数：公开无符号整数
    最近的正常事件延迟：公开可选<64位无符号整数>--没有正常事件时为空
    标准特征：公开trait实现
        签名：Clone for 事件快照

事件句柄<事件类型:事件特征>：结构体--统一封装事件节点，当前由pending事件创建流程返回给外部
    事件节点：事件节点--私有
    事件快照：共享引用<互斥锁<事件快照>>--私有，用于完成时同步更新快照
    类型标记：事件类型标记--私有
    MustUse：公开编译期警告--提醒调用者必须完成事件
    使用约束：由send_pending返回的事件句柄必须调用一次complete或complete_after并消费所有权，直接丢弃会留下永久pending事件

事件句柄<Result<T, E>>：实现
    约束：T与E满足Send + Sync + 'static
    完成：公开方法
        签名：complete(self, result: Result<T, E>)
        行为：调用complete_after并将额外延迟帧设为0
    延迟完成：公开方法
        签名：complete_after(self, result: Result<T, E>, 额外延迟帧：64位无符号整数)
        行为：
            锁定事件快照，锁中毒时终止并报告错误
            锁定事件节点，锁中毒时终止并报告错误
            如果事件状态不是pending，终止并报告CoreError::PendingEventAlreadyCompleted
            将事件状态从pending替换为正常
                事件体：使用result填充
                剩余延迟帧：额外延迟帧
            将pending事件数减1
            将正常事件数加1
            如果最近的正常事件延迟为空或额外延迟帧更小，将最近的正常事件延迟更新为额外延迟帧

事件队列：结构体--由World持有，对外不暴露字段
    待执行事件：循环数组<事件节点>--私有，因为需要大量首尾插入弹出操作
    事件快照：共享引用<互斥锁<事件快照>>--私有，与事件句柄共享
    构造：crate公开方法
        签名：new() -> Self
        行为：构造待执行事件为空、事件快照计数均为0且最近延迟为空的事件队列
    获取事件快照：公开方法
        签名：snapshot() -> 事件快照
        行为：锁定事件快照并返回克隆，锁中毒时终止并报告错误
    拉取事件：crate公开方法
        签名：pull_events(事件读取存储注册表可变引用)
        行为：
            锁定事件快照，锁中毒时终止并报告错误
            清空最近的正常事件延迟
            获取初始待执行事件数量
            循环：有界循环-遍历待执行事件，边界：次数-初始待执行事件数量
                弹出队首事件节点
                锁定事件节点，锁中毒时终止并报告错误
                分流：事件状态
                    pending：
                        释放事件节点锁
                        将事件节点推到队尾
                    正常且剩余延迟帧为0：
                        取出擦除事件体并调用擦除事件::push_into
                        如果对应事件读取存储不存在则自动创建，然后转移事件所有权
                        正常事件数减1
                    正常且剩余延迟帧大于0：
                        将剩余延迟帧减1
                        如果最近的正常事件延迟为空或当前剩余延迟帧更小，更新最近的正常事件延迟
                        释放事件节点锁
                        将事件节点推到队尾
    发送事件：公开泛型方法
        签名：send_event<事件类型:事件特征>(事件体：事件类型)
        行为：
            锁定事件快照，锁中毒时终止并报告错误
            创建状态为正常、事件体为传入事件、剩余延迟帧为0的事件节点并推入队尾
            正常事件数加1
            将最近的正常事件延迟更新为0
    延迟发送事件：公开泛型方法
        签名：send_event_after<事件类型:事件特征>(事件体：事件类型, 额外延迟帧：64位无符号整数)
        行为：
            锁定事件快照，锁中毒时终止并报告错误
            创建状态为正常、事件体为传入事件、剩余延迟帧为额外延迟帧的事件节点并推入队尾
            正常事件数加1
            如果最近的正常事件延迟为空或额外延迟帧更小，将最近的正常事件延迟更新为额外延迟帧
    发送pending事件：公开泛型方法
        签名：send_pending<T, E>() -> 事件句柄<Result<T, E>>
        约束：T与E满足Send + Sync + 'static
        行为：
            锁定事件快照，锁中毒时终止并报告错误
            创建状态为pending的事件节点
            事件队列与事件句柄分别持有一份事件节点共享引用
            pending事件数加1
            将事件节点推入队尾
            返回事件句柄
    快进：crate公开方法
        签名：fast_forward()
        行为：
            锁定事件快照，锁中毒时终止并报告错误
            如果最近的正常事件延迟为空或为0，直接返回
            遍历所有事件节点并逐个锁定，锁中毒时终止并报告错误
                状态为正常：将剩余延迟帧减去最近的正常事件延迟
                状态为pending：不处理
            将最近的正常事件延迟更新为0

事件特征：trait: Send + Sync + 'static

事件发射器：结构体--可克隆并跨线程发出事件，只写入Core事件队列，不唤醒Runtime
    事件队列：共享引用<写锁<事件队列>>--私有
    Clone：公开trait实现
    发出事件：公开泛型方法
        签名：emit_event<事件类型:事件特征>(&self, 事件体：事件类型)
        行为：获取事件队列写锁，调用事件队列::send_event后释放锁
    延迟发出事件：公开泛型方法
        签名：emit_event_after<事件类型:事件特征>(&self, 事件体：事件类型, 延迟帧：64位无符号整数)
        行为：获取事件队列写锁，调用事件队列::send_event_after后释放锁

事件读取器<'a, 事件类型:事件特征>：结构体--System临时持有
    事件：只读切片<'a, 事件类型>--私有，类型从未出现时为空切片，不持有锁守卫
    数量：公开方法
        签名：len() -> 无符号整数
        行为：返回读取器中的本次更新事件数量
    是否为空：公开方法
        签名：is_empty() -> 布尔值
        行为：返回读取器中是否没有本次更新事件
    进入迭代：公开trait实现
        签名：IntoIterator<Item = 事件类型引用> for 事件读取器<'a, 事件类型>
        行为：允许直接使用for 事件 in 读取器
    引用进入迭代：公开trait实现
        签名：IntoIterator<Item = 事件类型引用> for 事件读取器<'a, 事件类型>引用
        行为：允许直接迭代读取器引用并保留读取器
```

## 逻辑
```text
发送事件：
    World.emit_event(事件体)               -> 下一次tick
    World.emit_event_after(事件体, 0)      -> 下一次tick
    World.emit_event_after(事件体, frames) -> 下一次tick之后再等待frames次tick

发送pending事件：
    事件句柄 = World.emit_pending<Result成功类型, Result错误类型>()
    事件句柄.complete(result)               -> 下一次tick
    事件句柄.complete_after(result, frames) -> 下一次tick之后再等待frames次tick

读取事件：
    读取器 = World::获取事件读取器<事件类型>()--只访问本次更新事件
    --该类型从未发送过时读取器为空，不需要预先注册
    for 事件 in 读取器

每次tick开始：
    调用事件读取存储注册表::clear--只清空事件，保留存储
    调用事件队列::pull_events
        到期事件的所有权直接移入对应的事件读取存储
        未到期事件的剩余延迟帧减1并留在事件队列

快进tick：
    App::fast_forward_tick调用World::fast_forward_events
    事件队列将所有正常事件的剩余延迟帧减去最近的正常事件延迟
    App::fast_forward_tick调用App::tick
```

# World
## 类型
公开：
```text
World：结构体--统一持有并协调ECS数据
    构造：公开方法
        签名：new() -> Self
        行为：
            构造事件队列并放入写锁
            分别构造实体分配器、组件注册表、资源注册表、Query和事件读取存储注册表
            返回持有以上字段的World
    标准特征：公开trait实现
        签名：Default for World
        行为：调用World::new

    --实体与组件
    实体分配器：实体分配器--私有
    组件注册表：组件注册表--私有
    创建实体：公开方法
        签名：spawn() -> Entity
        行为：调用实体分配器::allocate并返回新Entity
    删除实体：公开方法
        签名：despawn(实体：Entity) -> 布尔值
        行为：
            如果实体分配器::is_alive返回假，返回假
            调用组件注册表::remove_entity删除该Entity的全部组件
            调用实体分配器::release回收Entity
            返回真
    检查实体存活：公开方法
        签名：is_alive(实体：Entity) -> 布尔值
        行为：返回实体分配器::is_alive的结果
    获取实体数量：公开方法
        签名：entity_count() -> 无符号整数
        行为：返回实体分配器::len的结果
    迭代存活实体：crate公开方法
        签名：entity_iter() -> 迭代器<Item = Entity>
        行为：返回实体分配器::iter_alive的结果，供Query构造排除类型的初始查询
    插入组件：公开泛型方法
        签名：insert_component<组件类型:组件>(实体：Entity, 组件：组件类型) -> 布尔值
        行为：
            如果实体分配器::is_alive返回假，丢弃组件并返回假
            调用组件注册表::insert
            返回真
    读取组件：公开泛型方法
        签名：get_component<组件类型:组件>(实体：Entity) -> 可选<组件类型只读引用>
        行为：
            如果实体分配器::is_alive返回假，返回空
            返回组件注册表::get的结果
    可变读取组件：公开泛型方法
        签名：get_component_mut<组件类型:组件>(实体：Entity) -> 可选<组件类型可变引用>
        行为：
            如果实体分配器::is_alive返回假，返回空
            返回组件注册表::get_mut的结果
    移除组件：公开泛型方法
        签名：remove_component<组件类型:组件>(实体：Entity) -> 可选<组件类型>
        行为：
            如果实体分配器::is_alive返回假，返回空
            返回组件注册表::remove的结果
    检查组件：公开泛型方法
        签名：contains_component<组件类型:组件>(实体：Entity) -> 布尔值
        行为：
            如果实体分配器::is_alive返回假，返回假
            返回组件注册表::contains的结果
    获取组件查询迭代器：crate公开泛型方法
        签名：query_iter<组件类型:组件>() -> 迭代器<Item = (Entity只读引用, 组件类型只读引用)>
        行为：返回组件注册表::iter的结果，供Query读取组件

    --查询
    查询器：Query--私有，无状态，由World统一提供查询入口
    查询包含组件的Entity：公开泛型方法
        签名：query_with<组件类型:组件>() -> QueryResult<'_>
        行为：将World只读引用传给Query::with并返回初始查询结果
    查询不包含组件的Entity：公开泛型方法
        签名：query_without<组件类型:组件>() -> QueryResult<'_>
        行为：将World只读引用传给Query::without并返回初始查询结果

    --资源
    资源注册表：资源注册表--私有
    插入资源：公开泛型方法
        签名：insert_resource<资源类型:Resource>(资源：资源类型)
        行为：调用资源注册表::insert注册或替换资源
    读取资源：公开泛型方法
        签名：get_resource<资源类型:Resource>() -> 可选<资源类型只读引用>
        行为：返回资源注册表::get的结果
    可变读取资源：公开泛型方法
        签名：get_resource_mut<资源类型:Resource>() -> 可选<资源类型可变引用>
        行为：返回资源注册表::get_mut的结果
    移除资源：公开泛型方法
        签名：remove_resource<资源类型:Resource>() -> 可选<资源类型>
        行为：返回资源注册表::remove的结果
    检查资源：公开泛型方法
        签名：contains_resource<资源类型:Resource>() -> 布尔值
        行为：返回资源注册表::contains的结果

    --事件
    事件队列：共享引用<写锁<事件队列>>--私有，与克隆出的事件发射器共享
    事件读取存储注册表：事件读取存储注册表--私有，tick开始时按到期事件类型自动扩展，System执行期共享读取
    发出事件：公开泛型方法
        签名：emit_event<事件类型:事件特征>(事件体：事件类型)
        行为：获取事件队列写锁，调用事件队列::send_event后释放锁
    延迟发出事件：公开泛型方法
        签名：emit_event_after<事件类型:事件特征>(事件体：事件类型, 延迟帧：64位无符号整数)
        行为：获取事件队列写锁，调用事件队列::send_event_after后释放锁
    发出pending事件：公开泛型方法
        签名：emit_pending<T, E>() -> 事件句柄<Result<T, E>>
        约束：T与E满足Send + Sync + 'static
        行为：获取事件队列写锁，调用事件队列::send_pending取得事件句柄后释放锁并返回事件句柄
    获取事件发射器：公开方法
        签名：event_emitter() -> 事件发射器
        行为：克隆事件队列共享引用并构造事件发射器
    获取事件读取器：公开泛型方法
        签名：event_reader<事件类型:事件特征>() -> 事件读取器<'_, 事件类型>
        行为：
            通过World只读引用访问事件读取存储注册表
            调用事件读取存储注册表::reader获取指定事件类型
            返回事件读取器
    获取事件快照：公开方法
        签名：event_snapshot() -> 事件快照
        行为：获取事件队列读锁并调用事件队列::snapshot返回快照克隆，锁中毒时终止并报告错误
    快进事件：公开方法
        签名：fast_forward_events()
        行为：获取事件队列写锁并调用事件队列::fast_forward，锁中毒时终止并报告错误

    更新事件：crate公开方法
        签名：tick()
        行为：
            条件：通过World可变引用调用
            获取事件队列写锁，锁中毒时终止并报告错误
            调用事件读取存储注册表::clear--清空读取存储
            调用事件队列::pull_events--装填到期事件并推进未到期事件的倒计时
            释放事件队列写锁
```

# Query
## 类型
公开：
```text
QueryResult<'world>：结构体--持有World共享借用和当前匹配Entity的只读查询结果
    查询结果：数组<Entity>--私有
    World：World只读引用<'world>--私有
    包含过滤：公开泛型方法
        签名：with<过滤组件类型:组件>() -> Self
        行为：
            保留查询结果中拥有过滤组件类型的Entity
            返回更新后的QueryResult
    排除过滤：公开泛型方法
        签名：without<过滤组件类型:组件>() -> Self
        行为：
            保留查询结果中不拥有过滤组件类型的Entity
            返回更新后的QueryResult
    取出结果：公开方法
        签名：result() -> 数组<Entity>
        行为：消耗QueryResult并返回查询结果，结束对World的共享借用
```

crate公开：
```text
Query：结构体--由World持有的无状态只读查询器
    构造：crate公开方法
        签名：new() -> Self
        行为：构造无状态Query
    包含过滤：crate公开泛型方法
        签名：with<过滤组件类型:组件>(World只读引用<'world>) -> QueryResult<'world>
        行为：通过World::query_iter收集所有拥有过滤组件类型的Entity并构造QueryResult
    排除过滤：crate公开泛型方法
        签名：without<过滤组件类型:组件>(World只读引用<'world>) -> QueryResult<'world>
        行为：通过World::entity_iter收集所有不拥有过滤组件类型的存活Entity并构造QueryResult
```

## 逻辑
```text
构造只读查询：
    World::query_with<组件类型>
        将World只读引用传给Query::with<组件类型>
        Query通过World::query_iter<组件类型>收集初始Entity
        返回持有World只读引用和初始Entity的QueryResult
    World::query_without<组件类型>
        将World只读引用传给Query::without<组件类型>
        Query通过World::entity_iter取得所有存活Entity
        排除拥有组件类型的Entity
        返回持有World只读引用和初始Entity的QueryResult

继续过滤只读查询：
    QueryResult::with<组件类型>保留拥有该组件的Entity
    QueryResult::without<组件类型>保留不拥有该组件的Entity
    每次过滤消耗旧QueryResult并返回更新后的QueryResult

取出只读查询结果：
    QueryResult::result消耗QueryResult
    返回数组<Entity>
    结束QueryResult对World的共享借用
```

# System
## 类型
公开：
```text
System：trait--对World执行一次同步逻辑的最小单元
    继承：Send + 'static
    执行：公开方法
        签名：run(World可变引用)
        行为：使用World执行一次System逻辑
    同步函数实现：公开泛型trait实现
        签名：System for F
        条件：F: FnMut(World可变引用) + Send + 'static
        行为：run调用该同步函数或闭包，并传入World可变引用
```

## 逻辑
```text
执行System：
    调度方取得World可变引用
    调用System::run
    System通过World公开API查询Entity并读写Component、Resource和Event
    System::run返回后结束对World的独占借用

约束：
    System只执行同步逻辑
    System不持有World
    System不负责Stage、排序、循环、帧推进、错误策略和异步任务
```

# Schedule
## 类型
crate公开：
```text
计划表：结构体--由App持有，组织单次Schedule和每帧Schedule的执行顺序
    首帧计划：数组<(布尔值, 无符号整数)>--布尔值表示是否单次执行，无符号整数表示对应Schedule下标
    首帧执行：数组<(字符串, Schedule)>
    每帧执行：数组<(字符串, Schedule)>
    是否已启动：布尔值
    执行逻辑：函数指针<(计划表可变引用, World可变引用)>--初始指向first_run，首帧执行后改为continued_run
    构造：crate公开方法
        签名：new() -> Self
        行为：构造三个数组均为空、尚未启动且执行逻辑指向first_run的计划表
    添加阶段：crate公开方法
        签名：add_schedule(阶段名：字符串) -> 布尔值
        行为：
            如果已经启动或check返回真，返回假
            将是否单次执行设为假，以每帧执行长度作为Schedule下标，推入首帧计划
            创建Schedule，与阶段名封装成元组，推入每帧执行
            返回真
    添加单次阶段：crate公开方法
        签名：add_once_schedule(阶段名：字符串) -> 布尔值
        行为：
            如果已经启动或check返回真，返回假
            将是否单次执行设为真，以首帧执行长度作为Schedule下标，推入首帧计划
            创建Schedule，与阶段名封装成元组，推入首帧执行
            返回真
    查询Schedule：crate公开方法
        签名：contains(阶段名：字符串引用) -> 布尔值
        行为：遍历首帧执行和每帧执行，存在同名阶段时返回真，否则返回假
    检查启动状态：crate公开方法
        签名：is_started() -> 布尔值
        行为：返回是否已启动
    获取阶段：crate公开方法
        签名：schedule_mut(阶段名：字符串引用) -> 可选<Schedule可变引用>
        行为：
            如果已经启动，返回空
            依次在首帧执行和每帧执行中查找同名阶段
            找到时返回Schedule可变引用，否则返回空
    执行首帧计划表：私有方法
        签名：first_run(World可变引用)
        行为：
            按首帧计划记录的类型和下标依次执行对应Schedule
            清空首帧计划
            清空首帧执行
            将是否已启动设为真
            将执行逻辑改为continued_run
    执行计划表：私有方法
        签名：continued_run(World可变引用)
        行为：按数组顺序执行每帧执行中的全部Schedule
    执行：crate公开方法
        签名：run(World可变引用)
        行为：调用执行逻辑指向的函数，并传入计划表和World可变引用
```

公开：
```text
Schedule：结构体--按注册顺序持有并执行System
    系统：数组<类型擦除<System>>--私有
    构造：公开方法
        签名：new() -> Self
        行为：构造不包含任何System的Schedule
    添加System：公开泛型方法
        签名：add_system<System类型:System>(system：System类型) -> &mut Self
        行为：擦除System具体类型并追加到System数组末尾，返回Schedule可变引用
    执行：公开方法
        签名：run(World可变引用)
        行为：按System数组顺序依次调用System::run，并为每个System传入同一个World可变引用
    标准特征：公开trait实现
        签名：Default for Schedule
        行为：调用Schedule::new
```

## 逻辑
```text
执行计划表：
    调用计划表::run
    run调用执行逻辑指向的函数
    第一次调用时执行逻辑指向first_run
        遍历首帧计划
        根据是否单次执行和Schedule下标，从对应数组取得Schedule
        调用Schedule::run
        清空首帧计划和首帧执行
        标记计划表已启动
        将执行逻辑改为continued_run
    后续调用时执行逻辑指向continued_run
        遍历每帧执行
        依次调用Schedule::run

执行Schedule：
    遍历System数组
    依次可变借用每个System
    调用System::run并传入World可变引用
    前一个System返回后再执行下一个System

约束：
    Schedule只按注册顺序串行执行System
    Schedule不负责Stage、循环、帧推进、排序依赖、并行执行、panic捕获和执行报告
```

# App
## 类型
公开：
```text
App：结构体--持有World和计划表的ECS组合根
    构造：公开方法
        签名：new() -> Self
        行为：构造World和计划表并返回App

    --world
    World：World--私有
    读取World：公开方法
        签名：world() -> World只读引用
        行为：返回World只读引用
    可变读取World：公开方法
        签名：world_mut() -> World可变引用
        行为：返回World可变引用

    --Plugin
    添加Plugin：公开泛型方法
        签名：add_plugin<Plugin类型:Plugin>(plugin：Plugin类型) -> &mut Self
        行为：
            如果计划表::is_started返回真，终止并报告CoreError::AppAlreadyStarted
            调用Plugin::build转移Plugin所有权并传入App可变引用
            Plugin配置完成后返回App可变引用

    --计划表
    计划表：计划表--私有
    添加阶段：公开方法
        签名：add_schedule(阶段名：字符串) -> &mut Self
        行为：
            调用计划表::add_schedule
            如果计划表已经启动，终止并报告CoreError::AppAlreadyStarted
            如果阶段重名，终止并报告CoreError::ScheduleAlreadyExists
            返回App可变引用
    添加单次阶段：公开方法
        签名：add_once_schedule(阶段名：字符串) -> &mut Self
        行为：
            调用计划表::add_once_schedule
            如果计划表已经启动，终止并报告CoreError::AppAlreadyStarted
            如果阶段重名，终止并报告CoreError::ScheduleAlreadyExists
            返回App可变引用
    查询Schedule：公开方法
        签名：contains_schedule(阶段名：字符串引用) -> 布尔值
        行为：调用计划表::contains并返回结果
    添加System：公开泛型方法
        签名：add_system<System类型:System>(阶段名：字符串引用, system：System类型) -> &mut Self
        行为：
            调用计划表::schedule_mut查找阶段
            如果计划表已经启动，终止并报告CoreError::AppAlreadyStarted
            如果阶段不存在，终止并报告CoreError::ScheduleNotFound
            调用Schedule::add_system转移System所有权
            返回App可变引用

    执行一次更新：公开方法
        签名：tick()
        行为：
            调用World::tick更新事件
            调用计划表::run执行本次更新的全部Schedule
    快进并执行一次更新：公开方法
        签名：fast_forward_tick()
        行为：
            调用World::fast_forward_events快进事件倒计时
            调用App::tick推进事件并执行全部Schedule
    标准特征：公开trait实现
        签名：Default for App
        行为：调用App::new
```

## 逻辑
```text
推进App：
    App::tick调用World::tick
    World完成事件倒计时推进和到期事件装填
    App::tick调用计划表::run
    计划表通过当前执行逻辑运行首帧计划或每帧Schedule
    Schedule依次执行System
    所有System执行完成后App::tick返回

快进App：
    App::fast_forward_tick调用World::fast_forward_events
    App::fast_forward_tick调用App::tick
```

# Plugin
## 类型
公开：
```text
Plugin：trait--在App启动前完成一组功能配置的一次性对象
    构建：公开方法
        签名：build(App可变引用)
        行为：取得Plugin所有权并配置App
    配置函数实现：公开泛型trait实现
        签名：Plugin for F
        条件：F: FnOnce(App可变引用)
        行为：build调用该配置函数或闭包，并传入App可变引用
```

## 逻辑
```text
挂载Plugin：
    App::add_plugin取得Plugin所有权
    如果App已经启动，终止并报告CoreError::AppAlreadyStarted
    调用Plugin::build并传入App可变引用
    Plugin通过App公开API注册Schedule、System以及World数据
    build返回后销毁Plugin，App不保存Plugin

约束：
    Plugin只负责启动前配置
    Plugin不参与每帧执行
    Plugin不负责依赖排序、卸载和运行时生命周期回调
```
