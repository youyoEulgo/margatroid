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

# MemoryPlugin

## 类型

公开：
```text
MemoryPlugin：Agent消息持久化插件，公开结构体--安装SQLite绑定与消息同步System
    schedule: String--持久化System所属Schedule，私有
    new() -> Self
        构造插件：公开关联函数，默认使用RuntimePlugin::UPDATE
    with_schedule(mut self, schedule: impl Into<String>) -> Self
        指定阶段：公开方法，替换默认Schedule并返回自身
    impl Default for MemoryPlugin
        Default：公开trait实现，与new等价
    impl Plugin for MemoryPlugin
        Plugin：公开trait实现
        build(self, app: &mut App)
            构建插件：安装SQLite持久化能力
            行为：
                在schedule中挂载sync_realtime_messages_system
                在schedule中挂载sync_history_resources_system

AgentMemory：Agent数据库绑定，公开组件--一个运行中AgentInstance独占一个SQLite连接
    path: PathBuf--WorkspacePlugin提供的规范化数据库路径，私有
    connection: Mutex<rusqlite::Connection>--只由MemoryPlugin持久化入口和System使用，私有
    open(path: impl Into<PathBuf>) -> Result<(Self, Vec<margatroid_types::Message>), MemoryError>
        打开记忆：公开关联函数，打开数据库并恢复实时上下文
        行为：
            创建数据库父目录
            打开SQLite文件
            调用initialize_schema创建不存在的业务表
            调用load_realtime_messages恢复当前动态消息
            返回尚未绑定Entity的AgentMemory和完整Vec<Message>
            任一步失败时不返回部分上下文
    path(&self) -> &Path
        取得路径：公开方法，返回数据库路径
    impl Component for AgentMemory
        Component：公开trait实现

AgentMemoryWriteFailed：消息持久化失败事件，公开结构体--报告历史或实时消息写入失败
    agent: Entity--写入失败的AgentInstance Entity
    error: MemoryError--不包含消息正文、资源正文或完整SQL语句的稳定错误
    impl Event for AgentMemoryWriteFailed
        Event：公开trait实现
    impl Clone for AgentMemoryWriteFailed
        Clone：公开trait实现

MemoryErrorKind：记忆错误分类，公开枚举
    InvalidPath
    DirectoryCreateFailed
    OpenFailed
    SchemaFailed
    ReadFailed
    DecodeFailed
    AgentNotAlive
    AgentMemoryMissing
    AlreadyBound
    PluginMissing
    WriteFailed
    impl Clone + Copy + PartialEq + Eq for MemoryErrorKind
        值语义：公开trait实现

MemoryError：记忆错误，公开结构体--保存稳定分类和有界描述
    kind: MemoryErrorKind--错误分类，私有
    message: String--不包含消息正文、资源正文或完整SQL语句的描述，私有
    kind(&self) -> MemoryErrorKind
        取得分类：公开方法
    message(&self) -> &str
        取得描述：公开方法
    impl Clone for MemoryError
        Clone：公开trait实现
    impl fmt::Display for MemoryError
        Display：公开trait实现，输出稳定错误描述
    impl std::error::Error for MemoryError
        Error：公开trait实现

WorldMemoryExt：World记忆扩展，公开trait--绑定Agent数据库并提供历史消息同步写入入口
    bind_agent_memory(
        &mut self,
        agent: Entity,
        memory: AgentMemory,
        messages: &[margatroid_types::Message],
    ) -> Result<(), MemoryError>
        绑定记忆：公开方法，将已打开的数据库绑定到Agent
        行为：
            MemoryPlugin必须已经安装
            agent必须存活
            agent不能已经持有AgentMemory
            为memory.connection开启事务
            调用rewrite_realtime_messages(transaction, messages)
            成功时提交事务并把memory插入agent
            重写或提交失败时恢复旧表且不插入部分绑定
    append_history_message(
        &mut self,
        event: &margatroid_types::AgentMessage,
    ) -> Result<(), MemoryError>
        追加历史消息：公开方法，由AgentPlugin同步调用
        行为：
            event.message是Message::Tool时直接返回成功，不访问数据库
            event.message只接受Message::User或Message::Assistant
            event.agent必须存活且持有AgentMemory
            为AgentMemory.connection开启事务
            调用insert_history_message(transaction, event, current_unix_milliseconds)
            成功时提交事务
            插入或提交失败时恢复旧表并返回MemoryError
```

## 函数

私有：
```text
initialize_schema(connection: &rusqlite::Connection) -> Result<(), MemoryError>
    初始化数据库：私有函数，创建不存在的两张业务表
    行为：
        创建history_messages表：
            sequence INTEGER PRIMARY KEY AUTOINCREMENT--单Agent永久递增序号
            turn_id TEXT NOT NULL--AgentMessage.id
            role TEXT NOT NULL--只保存user或assistant
            message TEXT NOT NULL--完整Message结构化JSON
            resources TEXT NOT NULL DEFAULT '[]'--MessageResource数组JSON，不保存资源正文
            created_at_ms INTEGER NOT NULL--写入时Unix毫秒时间
        创建realtime_messages表：
            position INTEGER PRIMARY KEY--从0开始的连续上下文位置
            message TEXT NOT NULL--完整Message结构化JSON
        任一建表操作失败时返回SchemaFailed

load_realtime_messages(
    connection: &rusqlite::Connection,
) -> Result<Vec<margatroid_types::Message>, MemoryError>
    恢复实时上下文：私有函数，读取当前Agent动态消息
    行为：
        按position升序读取全部realtime_messages
        position必须从0连续递增
        使用serde_json反序列化每行message
        不允许Message::System进入动态上下文
        任一位置缺口或非法JSON导致整体失败，不返回部分消息

rewrite_realtime_messages(
    transaction: &rusqlite::Transaction,
    messages: &[margatroid_types::Message],
) -> Result<(), MemoryError>
    重写实时上下文：私有函数，使realtime_messages与输入消息逐项完全一致
    行为：
        拒绝输入中的Message::System
        删除原realtime_messages全部行
        按messages顺序取得从0开始的position
        使用serde_json结构化序列化并逐行插入Message
        任一序列化或插入失败时返回WriteFailed，由外层事务恢复旧表

insert_history_message(
    transaction: &rusqlite::Transaction,
    event: &margatroid_types::AgentMessage,
    created_at_ms: i64,
) -> Result<(), MemoryError>
    插入历史消息：私有函数，向history_messages追加一条User或Assistant消息
    行为：
        event.message是Message::User时role使用user
        event.message是Message::Assistant时role使用assistant
        event.message是Message::Tool或Message::System时返回WriteFailed
        使用serde_json结构化序列化event.message
        使用event.id写入turn_id
        resources写入空数组
        使用created_at_ms写入创建时间
        插入失败时返回WriteFailed

merge_history_resources(
    transaction: &rusqlite::Transaction,
    turn_id: &str,
    resources: &[margatroid_types::MessageResource],
) -> Result<(), MemoryError>
    合并历史资源：私有函数，把本次实际使用的资源引用写入对应User历史行
    行为：
        按turn_id读取对应User历史行的resources
        使用serde_json反序列化现有MessageResource数组
        合并resources并去除重复ResourceRef
        使用serde_json序列化合并结果
        只更新该User历史行的resources列
        不写入Skill、Workflow或其他资源正文
        读取、序列化或更新失败时返回WriteFailed

sync_realtime_messages_system(world: &mut World)
    同步实时上下文：私有System，消费AgentContextMessagesUpdated并重写实时消息表
    行为：
        收集本次全部margatroid_types::AgentContextMessagesUpdated，结束对EventReader的借用
        按事件顺序处理每个事件，不合并同一Agent的连续事件
        agent不存在时发送AgentMemoryWriteFailed { kind: AgentNotAlive }
        agent没有AgentMemory时发送AgentMemoryWriteFailed { kind: AgentMemoryMissing }
        为AgentMemory.connection开启事务
        调用rewrite_realtime_messages(transaction, event.messages)
        成功时提交事务
        重写或提交失败时恢复旧表并发送AgentMemoryWriteFailed
        不重新读取AgentContext，不修改history_messages

sync_history_resources_system(world: &mut World)
    同步历史资源：私有System，消费AgentResourcesUsed并更新User历史行资源列
    行为：
        收集本次全部margatroid_types::AgentResourcesUsed，结束对EventReader的借用
        按事件顺序处理每个事件
        agent不存在时发送AgentMemoryWriteFailed { kind: AgentNotAlive }
        agent没有AgentMemory时发送AgentMemoryWriteFailed { kind: AgentMemoryMissing }
        为AgentMemory.connection开启事务
        调用merge_history_resources(transaction, &event.id, &event.resources)
        成功时提交事务
        合并或提交失败时恢复旧表并发送AgentMemoryWriteFailed
```

## 逻辑

```text
安装：
    app.add_plugin(MemoryPlugin)
        -> 保存持久化Schedule
        -> 挂载sync_realtime_messages_system
        -> 挂载sync_history_resources_system

Workspace启动Agent：
    WorkspacePlugin确定项目根、Workspace名称和Agent名称
        -> 默认数据库路径为<project>/.margatroid/workspaces/<workspace>/memory/<agent>/memory.sql
        -> 显式memory_path由WorkspacePlugin覆盖默认路径
        -> 调用AgentMemory::open(path)
        -> initialize_schema创建history_messages和realtime_messages
        -> load_realtime_messages恢复Vec<Message>
        -> 把Vec<Message>放入AgentCreateRequest.messages
        -> Agent创建后调用WorldMemoryExt::bind_agent_memory
    打开、建表或恢复失败
        -> Workspace启动该Agent失败
        -> 不使用空上下文替代不可读取的已有数据库

记录User或Assistant消息：
    AgentPlugin收到AgentMessage
        -> record_message识别Message::User或Message::Assistant
        -> 调用WorldMemoryExt::append_history_message
        -> insert_history_message向history_messages追加一行
        -> 历史事务提交成功
        -> AgentContext.append_message追加动态上下文
        -> 发送AgentContextMessagesUpdated完整快照
        -> sync_realtime_messages_system整体重写realtime_messages
    历史写入失败
        -> 不修改AgentContext

记录Tool响应：
    AgentPlugin收到Message::Tool
        -> 不调用WorldMemoryExt::append_history_message
        -> AgentContext.append_message追加动态上下文
        -> 发送AgentContextMessagesUpdated完整快照
        -> sync_realtime_messages_system整体重写realtime_messages
    Tool响应只进入AgentContext和realtime_messages，不进入history_messages

重写动态上下文：
    AgentContext.rewrite_messages替换完整消息数组
        -> 发送AgentContextMessagesUpdated完整快照
        -> sync_realtime_messages_system为每个事件开启独立事务
        -> rewrite_realtime_messages整体替换realtime_messages
    history_messages不删除、不覆盖，也不补写压缩结果

记录资源使用：
    SkillPlugin、WorkflowPlugin或其他资源Plugin发送AgentResourcesUsed
        -> 事件只携带ResourceRef，不携带资源正文
        -> sync_history_resources_system调用merge_history_resources
        -> 把资源引用合并到相同turn_id的User历史行
    Assistant历史行的resources保持空数组
    Tool响应没有历史行

Workspace重新启动：
    AgentMemory::open只从realtime_messages恢复AgentContext.messages
    history_messages保留User和Assistant历史，不自动装入当前模型上下文
    realtime_messages可以包含User、Assistant和Tool，不包含System

关闭：
    workspace down释放Agent Entity
        -> AgentMemory随组件释放
        -> SQLite连接关闭
        -> 数据库文件保留供下次workspace up或reload恢复

边界：
    WorkspacePlugin负责数据库路径、显式覆盖和Agent启动顺序
    AgentPlugin负责调用历史写入入口和修改AgentContext.messages
    资源Plugin只发送AgentResourcesUsed，不直接写SQLite
    MemoryPlugin不读取AgentImage、Workspace文件或资源正文
    MemoryPlugin不决定上下文压缩时机，不生成摘要，不实现记忆检索或向量索引
```

## 持有关系

```text
Workspace logical Agent
└── memory.sql
    ├── history_messages
    │   └── User / Assistant Message
    └── realtime_messages
        └── User / Assistant / Tool Message

World
├── margatroid_types::AgentMessage Event
│   └── AgentPlugin.record_message
│       ├── User / Assistant -> WorldMemoryExt::append_history_message
│       │   └── history_messages追加一行
│       ├── Tool -> 跳过history_messages
│       └── AgentContext.append_message
├── margatroid_types::AgentContextMessagesUpdated Event
│   └── sync_realtime_messages_system
│       └── realtime_messages整体重写
├── margatroid_types::AgentResourcesUsed Event
│   └── sync_history_resources_system
│       └── history_messages.resources合并资源引用
└── AgentInstance Entity
    ├── AgentContext
    │   └── messages: Vec<Message>
    └── AgentMemory
        ├── path: PathBuf
        └── connection: Mutex<rusqlite::Connection>
```
