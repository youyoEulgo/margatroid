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

函数：使用二级标题，按私有和公开分组，只放置不属于某个类型的操作
function_name<Generic>(parameter: ParameterType) -> ReturnType
    中文函数名：可见性函数，解释参数和用途
    约束：使用标准Rust where约束；单个约束写在同一行
    行为：展开完整逻辑

逻辑：使用二级标题，按执行顺序描述对象之间的调用关系
注释：字段注释使用--，类型、方法和函数的说明直接写在标题中
属性：不写Rust Attribute，实现时自行判断
边界：对外使用泛型和具体类型，内部使用类型擦除
```

# Error
## 类型
公开：
```text
CoreError：Core错误，公开枚举--描述core公开API能够识别的配置错误与状态错误
    AppAlreadyStarted
    ScheduleAlreadyExists { name: String }
    ScheduleNotFound { name: String }
    EntityCapacityExhausted
    PendingEventAlreadyCompleted
    panic(self) -> !
        终止：crate公开方法
        行为：使用自身Display描述触发panic
    impl fmt::Display for CoreError
        Display：公开trait实现
        fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result
            格式化错误：formatter接收稳定错误描述
            行为：输出包含错误类型与上下文的稳定错误描述
    impl std::error::Error for CoreError
        Error：公开trait实现
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
ErasedSparseColumn：擦除稀疏集合列，私有trait--擦除组件列的具体组件类型C，供ComponentRegistry统一持有
    继承：Any + Send + Sync + 'static
    remove_entity(&mut self, entity: Entity)
        移除实体：私有方法
        行为：移除并丢弃该Entity在当前列中的组件，不存在时不执行操作
    as_any(&self) -> &dyn Any
        转换为Any只读引用：私有方法
        行为：返回当前组件列的&dyn Any
    as_any_mut(&mut self) -> &mut dyn Any
        转换为Any可变引用：私有方法
        行为：返回当前组件列的&mut dyn Any
```

公开：
```text
Component：组件，公开trait--实体可持有的数据类型
    继承：Send + Sync + 'static
    实现：由开发者为实体数据类型显式实现
```

crate公开：
```text
SparseColumn<C: Component>：稀疏集合列，crate公开结构体--连续存储单一具体组件类型C，通过Entity索引定位稠密数据
    sparse: Vec<Option<usize>>--稀疏索引，Entity索引到稠密位置的映射
    entities: Vec<Entity>--实体，与稠密组件一一对应
    components: Vec<C>--组件，同一具体类型的连续稠密数据
    new() -> Self
        构造：crate公开方法
        行为：构造三个数组均为空的稀疏集合列
    dense_index(&self, entity: Entity) -> Option<usize>
        定位稠密位置：私有方法
        行为：
            使用Entity索引查找稀疏索引中的稠密位置
            如果位置不存在，返回空
            比较稠密位置中Entity的代数
            如果代数相同，返回稠密位置，否则返回空
    insert(&mut self, entity: Entity, component: C)
        插入组件：crate公开方法
        行为：
            如果dense_index返回稠密位置，替换该位置的组件并返回
            扩展稀疏索引以容纳Entity索引
            将新稠密位置写入稀疏索引
            将Entity和组件追加到对应稠密数组
    get(&self, entity: Entity) -> Option<&C>
        读取组件：crate公开方法
        行为：返回dense_index定位的组件只读引用，无法定位时返回空
    get_mut(&mut self, entity: Entity) -> Option<&mut C>
        可变读取组件：crate公开方法
        行为：返回dense_index定位的组件可变引用，无法定位时返回空
    iter(&self) -> impl Iterator<Item = (&Entity, &C)>
        迭代组件：crate公开方法
        行为：按稠密数组顺序成对迭代Entity和组件的只读引用
    remove(&mut self, entity: Entity) -> Option<C>
        移除组件：crate公开方法
        行为：
            如果dense_index返回空，返回空，否则记录待移除稠密位置
            清空待移除Entity的稀疏索引
            分别对实体数组和组件数组的相同位置执行swap_remove
            如果原末尾Entity被移动到待移除位置，更新该Entity在稀疏索引中的稠密位置
            返回被移除组件
    impl<C: Component> ErasedSparseColumn for SparseColumn<C>
        擦除存储：私有泛型trait实现
        remove_entity(&mut self, entity: Entity)
            移除实体：调用remove并丢弃返回的组件
        as_any(&self) -> &dyn Any
            转换为Any只读引用：返回自身的&dyn Any
        as_any_mut(&mut self) -> &mut dyn Any
            转换为Any可变引用：返回自身的&mut dyn Any

ComponentRegistry：组件注册表，crate公开结构体--管理所有组件类型C对应的SparseColumn，不负责检查Entity是否存活
    columns: HashMap<TypeId, Box<dyn ErasedSparseColumn>>--组件列
    new() -> Self
        构造：crate公开方法
        行为：创建不包含任何组件列的注册表
    insert<C: Component>(&mut self, entity: Entity, component: C)
        插入组件：crate公开泛型方法
        行为：
            获取C的TypeId
            获取对应的擦除稀疏集合列，不存在时创建并擦除稀疏集合列<C>
            将擦除稀疏集合列恢复为稀疏集合列<C>可变引用
            调用稀疏集合列<C>::insert
    get<C: Component>(&self, entity: Entity) -> Option<&C>
        读取组件：crate公开泛型方法
        行为：
            获取对应的擦除稀疏集合列
            将其恢复为稀疏集合列<C>只读引用
            返回稀疏集合列<C>::get的结果
            如果组件列不存在，返回空
    get_mut<C: Component>(&mut self, entity: Entity) -> Option<&mut C>
        可变读取组件：crate公开泛型方法
        行为：
            获取对应的擦除稀疏集合列
            将其恢复为稀疏集合列<C>可变引用
            返回稀疏集合列<C>::get_mut的结果
            如果组件列不存在，返回空
    iter<C: Component>(&self) -> impl Iterator<Item = (&Entity, &C)>
        迭代组件：crate公开泛型方法
        行为：恢复对应的稀疏集合列<C>并返回其只读迭代器，组件列不存在时返回空迭代器
    remove<C: Component>(&mut self, entity: Entity) -> Option<C>
        移除组件：crate公开泛型方法
        行为：
            获取对应的擦除稀疏集合列
            将其恢复为稀疏集合列<C>可变引用
            返回稀疏集合列<C>::remove的结果
            如果组件列不存在，返回空
    contains<C: Component>(&self, entity: Entity) -> bool
        检查组件：crate公开泛型方法
        行为：对应稀疏集合列<C>存在且能够定位该Entity时返回真，否则返回假
    remove_entity(&mut self, entity: Entity)
        移除实体的全部组件：crate公开方法
        行为：遍历所有擦除稀疏集合列并调用擦除稀疏集合列::remove_entity
    column<C: Component>(&self) -> Option<&SparseColumn<C>>
        恢复只读组件列：私有泛型方法
        行为：按C的TypeId取得擦除列并恢复为SparseColumn<C>，不存在时返回None
    column_mut<C: Component>(&mut self) -> Option<&mut SparseColumn<C>>
        恢复可变组件列：私有泛型方法
        行为：按C的TypeId取得擦除列并恢复为可变SparseColumn<C>，不存在时返回None
```

# Entity
## 类型
私有：
```text
EntitySlot：实体槽位，私有结构体--记录一个Entity索引的当前状态
    generation: u32--代数
    alive: bool--是否存活
```

公开：
```text
Entity：实体标识，公开元组结构体--带代数的实体标识，避免索引复用后旧标识指向新实体
    0: u64--标识，私有，高32位为代数，低32位为索引
    new(index: u32, generation: u32) -> Self
        构造：crate公开方法
        行为：将代数和索引组合为实体标识
    index(self) -> u32
        获取索引：公开方法
        行为：返回实体标识的低32位
    generation(self) -> u32
        获取代数：公开方法
        行为：返回实体标识的高32位
```

crate公开：
```text
EntityAllocator：实体分配器，crate公开结构体--分配和回收Entity标识
    slots: Vec<EntitySlot>--槽位，Entity索引对应的代数和存活状态
    free_indices: Vec<u32>--空闲索引，可以复用的Entity索引栈
    alive_count: usize--存活数量
    new() -> Self
        构造：crate公开方法
        行为：构造槽位和空闲索引均为空、存活数量为0的实体分配器
    allocate(&mut self) -> Entity
        分配：crate公开方法
        行为：
            如果空闲索引非空，弹出索引并将对应槽位标记为存活
            否则，使用槽位数量作为新索引
                如果新索引无法表示为u32，终止并报告CoreError::EntityCapacityExhausted
                追加代数为0且存活的槽位
            存活数量加1
            返回使用该索引和槽位当前代数构造的Entity
    release(&mut self, entity: Entity) -> bool
        回收：crate公开方法
        行为：
            如果is_alive返回假，返回假
            将对应槽位标记为不存活
            存活数量减1
            如果代数可以加1，推进代数并将索引推入空闲索引
            如果代数已达上限，永久退役该索引
            返回真
    is_alive(&self, entity: Entity) -> bool
        检查存活：crate公开方法
        行为：仅当索引存在、槽位存活且代数相同时返回真
    len(&self) -> usize
        获取存活数量：crate公开方法
        行为：返回存活数量
    iter_alive(&self) -> impl Iterator<Item = Entity> + '_
        迭代存活实体：crate公开方法
        行为：遍历存活的实体槽位，使用槽位索引和当前代数构造并返回Entity

```

# Resource
## 类型
公开：
```text
Resource：资源，公开trait--World持有的全局单例数据类型
    继承：Send + Sync + 'static
    实现：由开发者为World全局单例类型显式实现
```

crate公开：
```text
ResourceRegistry：资源注册表，crate公开结构体--按R持有全局单例
    resources: HashMap<TypeId, Box<dyn Any + Send + Sync>>--资源
    new() -> Self
        构造：crate公开方法
        行为：创建不包含任何资源的注册表
    insert<R: Resource>(&mut self, resource: R)
        插入资源：crate公开泛型方法
        行为：擦除资源具体类型并按TypeId插入；已有同类型资源时替换旧资源
    get<R: Resource>(&self) -> Option<&R>
        读取资源：crate公开泛型方法
        行为：获取对应的类型擦除资源，恢复为&R并返回，不存在时返回空
    get_mut<R: Resource>(&mut self) -> Option<&mut R>
        可变读取资源：crate公开泛型方法
        行为：获取对应的类型擦除资源，恢复为&mut R并返回，不存在时返回空
    remove<R: Resource>(&mut self) -> Option<R>
        移除资源：crate公开泛型方法
        行为：移除对应的类型擦除资源，恢复为具体R并返回，不存在时返回空
    contains<R: Resource>(&self) -> bool
        检查资源：crate公开泛型方法
        行为：对应R的TypeId存在时返回真，否则返回假
```

# Event
## 类型
私有：
```text
EventState：事件状态，私有枚举
    Pending
    Normal {
        body: Box<dyn ErasedEvent>--事件体
        remaining_delay_frames: u64--剩余延迟帧
    }

EventNode：事件节点，私有类型别名--等于Arc<Mutex<EventState>>

ErasedEvent：擦除事件，私有trait--跨线程排队时擦除具体类型，到期后在主线程恢复对应存储操作
    push_into(self: Box<Self>, registry: &mut EventReadStorageRegistry)
        装填读取存储：私有方法
        行为：由具体事件类型E实现，调用registry.push转移事件所有权
    impl<E: Event> ErasedEvent for E
        事件类型E实现：私有泛型trait实现
        push_into(self: Box<Self>, registry: &mut EventReadStorageRegistry)
            装填读取存储：恢复事件所有权并调用registry.push

EventReadStorage<E: Event>：事件读取存储，私有结构体--持有单一类型的本次更新事件
    events: Vec<E>--事件
    new() -> Self
        构造：私有关联函数
        行为：构造事件数组为空的读取存储
    push(&mut self, event: E)
        装填事件：私有方法
        行为：取得事件体所有权并推入事件数组
    impl<E: Event> ErasedEventReadStorage for EventReadStorage<E>
        擦除存储：私有trait实现
        clear(&mut self)
            清空：清空事件数组
        as_any(&self) -> &dyn Any
            转换为Any只读引用：返回自身的&dyn Any
        as_any_mut(&mut self) -> &mut dyn Any
            转换为Any可变引用：返回自身的&mut dyn Any

ErasedEventReadStorage：擦除事件读取存储，私有trait
    clear()
        清空：私有方法
    as_any() -> &dyn Any
        转换为Any只读引用：私有方法
    as_any_mut() -> &mut dyn Any
        转换为Any可变引用：私有方法
```

crate公开：
```text
EventReadStorageRegistry：事件读取存储注册表，crate公开结构体--由World持有
    storages: HashMap<TypeId, Box<dyn ErasedEventReadStorage>>--读取存储
    new() -> Self
        构造：crate公开方法
        行为：创建不包含任何事件读取存储的注册表
    reader<E: Event>(&self) -> EventReader<'_, E>
        获取事件读取器：crate公开泛型方法
        行为：
            类型已出现时返回持有对应事件切片的读取器
            类型从未出现时返回持有空切片的读取器
    push<E: Event>(&mut self, event: E)
        装填事件：私有方法
        行为：
            如果对应类型存储不存在，创建事件读取存储<E>
            将存储恢复为事件读取存储<E>
            调用事件读取存储::push转移事件所有权
    clear(&mut self)
        清空：crate公开方法
        行为：
            遍历所有事件读取存储
            调用事件读取存储::clear，保留已创建的存储
```

公开：
```text
impl<T, E> Event for Result<T, E>
    Result事件：公开泛型trait实现
    约束：T: Send + Sync + 'static，E: Send + Sync + 'static
    行为：允许Result<T, E>作为事件发送和读取

EventSnapshot：事件快照，公开结构体--事件队列持有，对外只返回字段可读的克隆
    normal_event_count: usize--正常事件数，公开
    pending_event_count: usize--pending事件数，公开
    nearest_normal_event_delay: Option<u64>--最近正常事件延迟，公开

EventHandle<E: Event>：事件句柄，公开结构体--统一封装事件节点，当前由pending事件创建流程返回给外部
    node: EventNode--事件节点，私有
    snapshot: Arc<Mutex<EventSnapshot>>--事件快照，私有
    marker: PhantomData<E>--类型标记，私有
    使用约束：由send_pending返回的事件句柄必须调用一次complete或complete_after并消费所有权，直接丢弃会留下永久pending事件

EventHandle<Result<T, E>>：Result事件句柄，公开泛型实现
    约束：T: Send + Sync + 'static，E: Send + Sync + 'static
    complete(self, result: Result<T, E>)
        完成：公开方法
        行为：调用complete_after并将额外延迟帧设为0
    complete_after(self, result: Result<T, E>, extra_delay_frames: u64)
        延迟完成：公开方法
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

EventQueue：事件队列，公开结构体--由World持有，对外不暴露字段
    pending: VecDeque<EventNode>--待执行事件，私有
    snapshot: Arc<Mutex<EventSnapshot>>--事件快照，私有
    new() -> Self
        构造：crate公开方法
        行为：构造待执行事件为空、事件快照计数均为0且最近延迟为空的事件队列
    snapshot(&self) -> EventSnapshot
        获取事件快照：公开方法
        行为：锁定事件快照并返回克隆，锁中毒时终止并报告错误
    pull_events(&mut self, registry: &mut EventReadStorageRegistry)
        拉取事件：crate公开方法
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
    send_event<E: Event>(&mut self, event: E)
        发送事件：公开泛型方法
        行为：
            锁定事件快照，锁中毒时终止并报告错误
            创建状态为正常、事件体为传入事件、剩余延迟帧为0的事件节点并推入队尾
            正常事件数加1
            将最近的正常事件延迟更新为0
    send_event_after<E: Event>(&mut self, event: E, extra_delay_frames: u64)
        延迟发送事件：公开泛型方法
        行为：
            锁定事件快照，锁中毒时终止并报告错误
            创建状态为正常、事件体为传入事件、剩余延迟帧为额外延迟帧的事件节点并推入队尾
            正常事件数加1
            如果最近的正常事件延迟为空或额外延迟帧更小，将最近的正常事件延迟更新为额外延迟帧
    send_pending<T, E>(&mut self) -> EventHandle<Result<T, E>>
        发送pending事件：公开泛型方法
        约束：T: Send + Sync + 'static，E: Send + Sync + 'static
        行为：
            锁定事件快照，锁中毒时终止并报告错误
            创建状态为pending的事件节点
            事件队列与事件句柄分别持有一份事件节点共享引用
            pending事件数加1
            将事件节点推入队尾
            返回事件句柄
    fast_forward(&mut self)
        快进：crate公开方法
        行为：
            锁定事件快照，锁中毒时终止并报告错误
            如果最近的正常事件延迟为空或为0，直接返回
            遍历所有事件节点并逐个锁定，锁中毒时终止并报告错误
                状态为正常：将剩余延迟帧减去最近的正常事件延迟
                状态为pending：不处理
            将最近的正常事件延迟更新为0

Event：事件，公开trait--继承Send + Sync + 'static

EventEmitter：事件发射器，公开结构体--可克隆并跨线程发出事件，只写入Core事件队列，不唤醒Runtime
    queue: Arc<RwLock<EventQueue>>--事件队列，私有
    new(queue: Arc<RwLock<EventQueue>>) -> Self
        构造发射器：crate公开关联函数，queue是共享事件队列
        行为：持有queue
    emit_event<E: Event>(&self, event: E)
        发出事件：公开泛型方法
        行为：获取事件队列写锁，调用事件队列::send_event后释放锁
    emit_event_after<E: Event>(&self, event: E, delay: u64)
        延迟发出事件：公开泛型方法
        行为：获取事件队列写锁，调用事件队列::send_event_after后释放锁

EventReader<'a, E: Event>：事件读取器，公开结构体--System临时持有
    events: &'a [E]--事件，私有，类型从未出现时为空切片
    len(&self) -> usize
        数量：公开方法
        行为：返回读取器中的本次更新事件数量
    is_empty(&self) -> bool
        是否为空：公开方法
        行为：返回读取器中是否没有本次更新事件
    impl<'a, E: Event> IntoIterator for EventReader<'a, E>
        进入迭代：公开trait实现
        关联类型：Item = &'a E，IntoIter = std::slice::Iter<'a, E>
        into_iter(self) -> Self::IntoIter
            进入迭代：返回events的切片迭代器，允许直接使用for 事件 in 读取器
    impl<'reader, 'storage, E: Event> IntoIterator for &'reader EventReader<'storage, E>
        引用进入迭代：公开trait实现
        关联类型：Item = &'reader E，IntoIter = std::slice::Iter<'reader, E>
        into_iter(self) -> Self::IntoIter
            引用进入迭代：返回events的切片迭代器，允许保留读取器
```

## 逻辑
```text
发送事件：
    World::emit_event(事件体)               -> 下一次tick
    World::emit_event_after(事件体, 0)      -> 下一次tick
    World::emit_event_after(事件体, frames) -> 下一次tick之后再等待frames次tick

发送pending事件：
    事件句柄 = World::emit_pending<Result成功类型, Result错误类型>()
    事件句柄.complete(result)               -> 下一次tick
    事件句柄.complete_after(result, frames) -> 下一次tick之后再等待frames次tick

读取事件：
    读取器 = World::获取事件读取器<E>()--只访问本次更新事件
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
World：ECS世界，公开结构体--统一持有并协调ECS数据
    entities: EntityAllocator--实体分配器，私有
    components: ComponentRegistry--组件注册表，私有
    resources: ResourceRegistry--资源注册表，私有
    query: Query--查询器，私有
    event_queue: Arc<RwLock<EventQueue>>--事件队列，私有
    event_registry: EventReadStorageRegistry--事件读取存储注册表，私有
    new() -> Self
        构造：公开方法
        行为：
            构造事件队列并放入写锁
            分别构造实体分配器、组件注册表、资源注册表、Query和事件读取存储注册表
            返回持有以上字段的World
    impl Default for World
        Default：公开trait实现
        default() -> Self
            构造默认World：调用World::new
            行为：返回新World

    --实体与组件
    spawn(&mut self) -> Entity
        创建实体：公开方法
        行为：调用实体分配器::allocate并返回新Entity
    despawn(&mut self, entity: Entity) -> bool
        删除实体：公开方法
        行为：
            如果实体分配器::is_alive返回假，返回假
            调用组件注册表::remove_entity删除该Entity的全部组件
            调用实体分配器::release回收Entity
            返回真
    is_alive(&self, entity: Entity) -> bool
        检查实体存活：公开方法
        行为：返回实体分配器::is_alive的结果
    entity_count(&self) -> usize
        获取实体数量：公开方法
        行为：返回实体分配器::len的结果
    entity_iter(&self) -> impl Iterator<Item = Entity> + '_
        迭代存活实体：crate公开方法
        行为：返回实体分配器::iter_alive的结果，供Query构造排除类型的初始查询
    insert_component<C: Component>(&mut self, entity: Entity, component: C) -> bool
        插入组件：公开泛型方法
        行为：
            如果实体分配器::is_alive返回假，丢弃组件并返回假
            调用组件注册表::insert
            返回真
    get_component<C: Component>(&self, entity: Entity) -> Option<&C>
        读取组件：公开泛型方法
        行为：
            如果实体分配器::is_alive返回假，返回空
            返回组件注册表::get的结果
    get_component_mut<C: Component>(&mut self, entity: Entity) -> Option<&mut C>
        可变读取组件：公开泛型方法
        行为：
            如果实体分配器::is_alive返回假，返回空
            返回组件注册表::get_mut的结果
    remove_component<C: Component>(&mut self, entity: Entity) -> Option<C>
        移除组件：公开泛型方法
        行为：
            如果实体分配器::is_alive返回假，返回空
            返回组件注册表::remove的结果
    contains_component<C: Component>(&self, entity: Entity) -> bool
        检查组件：公开泛型方法
        行为：
            如果实体分配器::is_alive返回假，返回假
            返回组件注册表::contains的结果
    query_iter<C: Component>(&self) -> impl Iterator<Item = (&Entity, &C)>
        获取组件查询迭代器：crate公开泛型方法
        行为：返回组件注册表::iter的结果，供Query读取组件

    --查询
    query_with<C: Component>(&self) -> QueryResult<'_>
        查询包含组件的Entity：公开泛型方法
        行为：将&World传给Query::with并返回初始查询结果
    query_without<C: Component>(&self) -> QueryResult<'_>
        查询不包含组件的Entity：公开泛型方法
        行为：将&World传给Query::without并返回初始查询结果

    --资源
    insert_resource<R: Resource>(&mut self, resource: R)
        插入资源：公开泛型方法
        行为：调用资源注册表::insert注册或替换资源
    get_resource<R: Resource>(&self) -> Option<&R>
        读取资源：公开泛型方法
        行为：返回资源注册表::get的结果
    get_resource_mut<R: Resource>(&mut self) -> Option<&mut R>
        可变读取资源：公开泛型方法
        行为：返回资源注册表::get_mut的结果
    remove_resource<R: Resource>(&mut self) -> Option<R>
        移除资源：公开泛型方法
        行为：返回资源注册表::remove的结果
    contains_resource<R: Resource>(&self) -> bool
        检查资源：公开泛型方法
        行为：返回资源注册表::contains的结果

    --事件
    emit_event<E: Event>(&self, event: E)
        发出事件：公开泛型方法
        行为：获取事件队列写锁，调用事件队列::send_event后释放锁
    emit_event_after<E: Event>(&self, event: E, delay: u64)
        延迟发出事件：公开泛型方法
        行为：获取事件队列写锁，调用事件队列::send_event_after后释放锁
    emit_pending<T, E>(&self) -> EventHandle<Result<T, E>>
        发出pending事件：公开泛型方法
        约束：T: Send + Sync + 'static，E: Send + Sync + 'static
        行为：获取事件队列写锁，调用事件队列::send_pending取得事件句柄后释放锁并返回事件句柄
    event_emitter(&self) -> EventEmitter
        获取事件发射器：公开方法
        行为：克隆事件队列共享引用并构造事件发射器
    event_reader<E: Event>(&self) -> EventReader<'_, E>
        获取事件读取器：公开泛型方法
        行为：
            通过&World访问事件读取存储注册表
            调用事件读取存储注册表::reader获取指定E
            返回事件读取器
    event_snapshot(&self) -> EventSnapshot
        获取事件快照：公开方法
        行为：获取事件队列读锁并调用事件队列::snapshot返回快照克隆，锁中毒时终止并报告错误
    fast_forward_events(&self)
        快进事件：公开方法
        行为：获取事件队列写锁并调用事件队列::fast_forward，锁中毒时终止并报告错误

    tick(&mut self)
        更新事件：crate公开方法
        行为：
            条件：通过&mut World调用
            获取事件队列写锁，锁中毒时终止并报告错误
            调用事件读取存储注册表::clear--清空读取存储
            调用事件队列::pull_events--装填到期事件并推进未到期事件的倒计时
            释放事件队列写锁
```

# Query
## 类型
公开：
```text
QueryResult<'world>：查询结果，公开结构体--持有World共享借用和当前匹配Entity
    world: &'world World--World共享借用，私有
    entities: Vec<Entity>--查询结果，私有
    with<C: Component>(mut self) -> Self
        包含过滤：公开泛型方法
        行为：
            保留查询结果中拥有过滤组件类型C的Entity
            返回更新后的QueryResult
    without<C: Component>(mut self) -> Self
        排除过滤：公开泛型方法
        行为：
            保留查询结果中不拥有过滤组件类型C的Entity
            返回更新后的QueryResult
    result(self) -> Vec<Entity>
        取出结果：公开方法
        行为：消耗QueryResult并返回查询结果，结束对World的共享借用
```

crate公开：
```text
Query：查询器，crate公开单元结构体--由World持有的无状态只读查询器
    new() -> Self
        构造：crate公开方法
        行为：构造无状态Query
    with<'world, C: Component>(&self, world: &'world World) -> QueryResult<'world>
        包含过滤：crate公开泛型方法
        行为：通过World::query_iter收集所有拥有组件类型C的Entity并构造QueryResult
    without<'world, C: Component>(&self, world: &'world World) -> QueryResult<'world>
        排除过滤：crate公开泛型方法
        行为：通过World::entity_iter收集所有不拥有组件类型C的存活Entity并构造QueryResult
```

## 逻辑
```text
构造只读查询：
    World::query_with<C>
        将&World传给Query::with<C>
        Query通过World::query_iter<C>收集初始Entity
        返回持有&World和初始Entity的QueryResult
    World::query_without<C>
        将&World传给Query::without<C>
        Query通过World::entity_iter取得所有存活Entity
        排除拥有C的Entity
        返回持有&World和初始Entity的QueryResult

继续过滤只读查询：
    QueryResult::with<C>保留拥有该组件的Entity
    QueryResult::without<C>保留不拥有该组件的Entity
    每次过滤消耗旧QueryResult并返回更新后的QueryResult

取出只读查询结果：
    QueryResult::result消耗QueryResult
    返回Vec<Entity>
    结束QueryResult对World的共享借用
```

# System
## 类型
公开：
```text
System：系统，公开trait--对World执行一次同步逻辑的最小单元
    继承：Send + 'static
    run(&mut self, world: &mut World)
        执行：公开方法
        行为：使用World执行一次System逻辑
    impl<F> System for F
        同步函数实现：公开泛型trait实现
        约束：F: FnMut(&mut World) + Send + 'static
        run(&mut self, world: &mut World)
            执行：调用该同步函数或闭包，并传入world
```

## 逻辑
```text
执行System：
    调度方取得&mut World
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
SchedulePlan：计划表，crate公开结构体--由App持有，组织单次Schedule和每帧Schedule的执行顺序
    first_plan: Vec<(bool, usize)>--首帧计划，bool表示是否单次执行
    once: Vec<(String, Schedule)>--单次Schedule
    recurring: Vec<(String, Schedule)>--每帧Schedule
    started: bool--是否已启动
    run_plan: RunPlan--执行逻辑，初始指向first_run
    new() -> Self
        构造：crate公开方法
        行为：构造三个数组均为空、尚未启动且执行逻辑指向first_run的计划表
    add_schedule(&mut self, name: String) -> bool
        添加阶段：crate公开方法
        行为：
            如果已经启动或check返回真，返回假
            将是否单次执行设为假，以每帧执行长度作为Schedule下标，推入首帧计划
            创建Schedule，与阶段名封装成元组，推入每帧执行
            返回真
    add_once_schedule(&mut self, name: String) -> bool
        添加单次阶段：crate公开方法
        行为：
            如果已经启动或check返回真，返回假
            将是否单次执行设为真，以首帧执行长度作为Schedule下标，推入首帧计划
            创建Schedule，与阶段名封装成元组，推入首帧执行
            返回真
    contains(&self, name: &str) -> bool
        查询Schedule：crate公开方法
        行为：遍历首帧执行和每帧执行，存在同名阶段时返回真，否则返回假
    is_started(&self) -> bool
        检查启动状态：crate公开方法
        行为：返回是否已启动
    schedule_mut(&mut self, name: &str) -> Option<&mut Schedule>
        获取阶段：crate公开方法
        行为：
            如果已经启动，返回空
            依次在首帧执行和每帧执行中查找同名阶段
            找到时返回Schedule可变引用，否则返回空
    first_run(&mut self, world: &mut World)
        执行首帧计划表：私有方法
        行为：
            按首帧计划记录的类型和下标依次执行对应Schedule
            清空首帧计划
            清空首帧执行
            将是否已启动设为真
            将执行逻辑改为continued_run
    continued_run(&mut self, world: &mut World)
        执行计划表：私有方法
        行为：按数组顺序执行每帧执行中的全部Schedule
    run(&mut self, world: &mut World)
        执行：crate公开方法
        行为：调用执行逻辑指向的函数，并传入计划表和&mut World
```

公开：
```text
Schedule：调度阶段，公开结构体--按注册顺序持有并执行System
    systems: Vec<Box<dyn System>>--系统，私有
    new() -> Self
        构造：公开方法
        行为：构造不包含任何System的Schedule
    add_system<S: System>(&mut self, system: S) -> &mut Self
        添加System：公开泛型方法
        行为：擦除System具体类型并追加到System数组末尾，返回Schedule可变引用
    run(&mut self, world: &mut World)
        执行：公开方法
        行为：按System数组顺序依次调用System::run，并为每个System传入同一个&mut World
    impl Default for Schedule
        Default：公开trait实现
        default() -> Self
            构造默认Schedule：调用Schedule::new
            行为：返回空Schedule
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
    调用System::run并传入&mut World
    前一个System返回后再执行下一个System

约束：
    Schedule只按注册顺序串行执行System
    Schedule不负责Stage、循环、帧推进、排序依赖、并行执行、panic捕获和执行报告
```

# App
## 类型
公开：
```text
App：应用，公开结构体--持有World和计划表的ECS组合根
    world: World--World，私有
    schedules: SchedulePlan--计划表，私有
    new() -> Self
        构造：公开方法
        行为：构造World和计划表并返回App

    --world
    world(&self) -> &World
        读取World：公开方法
        行为：返回&World
    world_mut(&mut self) -> &mut World
        可变读取World：公开方法
        行为：返回&mut World

    --Plugin
    add_plugin<P: Plugin>(&mut self, plugin: P) -> &mut Self
        添加Plugin：公开泛型方法
        行为：
            如果计划表::is_started返回真，终止并报告CoreError::AppAlreadyStarted
            调用Plugin::build转移Plugin所有权并传入App可变引用
            Plugin配置完成后返回App可变引用

    --计划表
    add_schedule(&mut self, name: String) -> &mut Self
        添加阶段：公开方法
        行为：
            调用计划表::add_schedule
            如果计划表已经启动，终止并报告CoreError::AppAlreadyStarted
            如果阶段重名，终止并报告CoreError::ScheduleAlreadyExists
            返回App可变引用
    add_once_schedule(&mut self, name: String) -> &mut Self
        添加单次阶段：公开方法
        行为：
            调用计划表::add_once_schedule
            如果计划表已经启动，终止并报告CoreError::AppAlreadyStarted
            如果阶段重名，终止并报告CoreError::ScheduleAlreadyExists
            返回App可变引用
    contains_schedule(&self, name: &str) -> bool
        查询Schedule：公开方法
        行为：调用计划表::contains并返回结果
    add_system<S: System>(&mut self, schedule: &str, system: S) -> &mut Self
        添加System：公开泛型方法
        行为：
            调用计划表::schedule_mut查找阶段
            如果计划表已经启动，终止并报告CoreError::AppAlreadyStarted
            如果阶段不存在，终止并报告CoreError::ScheduleNotFound
            调用Schedule::add_system转移System所有权
            返回App可变引用

    tick(&mut self)
        执行一次更新：公开方法
        行为：
            调用World::tick更新事件
            调用计划表::run执行本次更新的全部Schedule
    fast_forward_tick(&mut self)
        快进并执行一次更新：公开方法
        行为：
            调用World::fast_forward_events快进事件倒计时
            调用App::tick推进事件并执行全部Schedule
    impl Default for App
        Default：公开trait实现
        default() -> Self
            构造默认App：调用App::new
            行为：返回新App
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
Plugin：插件，公开trait--在App启动前完成一组功能配置的一次性对象
    build(self, app: &mut App)
        构建：公开方法
        行为：取得Plugin所有权并配置App
    impl<F> Plugin for F
        配置函数实现：公开泛型trait实现
        约束：F: FnOnce(&mut App)
        build(self, app: &mut App)
            构建：调用该配置函数或闭包，并传入app
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
