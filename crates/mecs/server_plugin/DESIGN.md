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
注释：统一使用--，可以附在对象后或单独成一行
边界：对外使用泛型和具体类型，内部使用类型擦除
```

# ServerPlugin

设计原则：同步处理，异步累积；事件传递状态，通道传递连续数据。

crate名称：server_plugin。

## 类型

公开：
```text
HTTP完整响应：类型别名<http::Response<Bytes>>--普通非流式响应的便捷类型

HTTP响应头：结构体--流式响应开始时一次性确定的HTTP状态与响应头
    状态码：StatusCode--私有，默认200
    响应头：HeaderMap--私有
    默认：公开trait实现
        签名：Default for HTTP响应头
        行为：构造状态码为200且没有额外响应头的HTTP响应头
    设置状态码：公开方法
        签名：with_status(self, status: StatusCode) -> Self
        行为：覆盖状态码并返回自身
    设置响应头：公开方法
        签名：with_header(self, name: HeaderName, value: HeaderValue) -> Self
        行为：写入响应头并返回自身

HTTP响应错误：枚举
    NonExhaustive：公开属性--允许后续版本增加错误变体
    ResponseAlreadyStarted
    StreamNotStarted
    ResponseClosed
    RequestClosed
    Debug：公开trait实现
        签名：Debug for HTTP响应错误
    Display：公开trait实现
        签名：Display for HTTP响应错误
        行为：输出响应状态与失败操作的稳定描述
    Error：公开trait实现
        签名：Error for HTTP响应错误

HTTP流错误：结构体--无法继续完成响应流时交给Axum Body的错误
    错误信息：字符串--私有
    使用消息构造：公开方法
        签名：new(message: impl Into<字符串>) -> Self
        行为：保存错误信息并返回自身
    Debug：公开trait实现
        签名：Debug for HTTP流错误
    Display：公开trait实现
        签名：Display for HTTP流错误
        行为：输出错误信息
    Error：公开trait实现
        签名：Error for HTTP流错误

服务器错误：枚举
    NonExhaustive：公开属性--允许后续版本增加错误变体
    RuntimePluginMissing
    AsyncRuntimePluginMissing
    ServerPluginAlreadyInstalled
    ServerPluginMissing
    RoutesFrozen
    EventRouteAlreadyRegistered { method: Method, path: 字符串 }
    WebSocketRouteAlreadyRegistered { path: 字符串 }
    UnsupportedMethod { method: Method }
    InvalidBodyLimit { limit: 无符号整数 }
    InvalidResponseStartTimeout { timeout: Duration }
    InvalidStreamBufferCapacity { capacity: 无符号整数 }
    InvalidWebSocketBufferCapacity { capacity: 无符号整数 }
    InvalidShutdownTimeout { timeout: Duration }
    终止：crate公开方法
        签名：panic(self) -> Never
        行为：使用自身Display描述触发panic
    Debug：公开trait实现
        签名：Debug for 服务器错误
    Display：公开trait实现
        签名：Display for 服务器错误
        行为：输出包含错误类型与上下文的稳定错误描述
    Error：公开trait实现
        签名：Error for 服务器错误

服务器配置：结构体
    监听地址：SocketAddr--私有，默认127.0.0.1:3939
    请求体上限：无符号整数--私有，仅限制事件委托路由完整缓冲的请求体
    响应开始超时：Duration--私有，仅限制System开始完整响应或流式响应所需时间
    流缓冲容量：无符号整数--私有，限制每条流尚未被客户端消费的分片数量
    WebSocket通道容量：无符号整数--私有，同时限制连接发送通道和每条接收支流的待处理消息数量
    优雅停止超时：Duration--私有
    Resource：公开trait实现
    Clone：公开trait实现
    默认：公开trait实现
        签名：Default for 服务器配置
        行为：构造监听127.0.0.1:3939并使用安全默认限制的配置
    设置监听地址：公开方法
        签名：with_bind(self, address: SocketAddr) -> Self
        行为：覆盖监听地址并返回自身
    设置请求体上限：公开方法
        签名：with_body_limit(self, limit: 无符号整数) -> Self
        行为：limit为0时终止并报告InvalidBodyLimit，否则覆盖请求体上限并返回自身
    设置响应开始超时：公开方法
        签名：with_response_start_timeout(self, timeout: Duration) -> Self
        行为：timeout为0时终止并报告InvalidResponseStartTimeout，否则覆盖响应开始超时并返回自身
    设置流缓冲容量：公开方法
        签名：with_stream_buffer_capacity(self, capacity: 无符号整数) -> Self
        行为：capacity为0时终止并报告InvalidStreamBufferCapacity，否则覆盖流缓冲容量并返回自身
    设置WebSocket通道容量：公开方法
        签名：with_websocket_buffer_capacity(self, capacity: 无符号整数) -> Self
        行为：capacity为0时终止并报告InvalidWebSocketBufferCapacity，否则覆盖WebSocket通道容量并返回自身
    设置优雅停止超时：公开方法
        签名：with_shutdown_timeout(self, timeout: Duration) -> Self
        行为：timeout为0时终止并报告InvalidShutdownTimeout，否则覆盖优雅停止超时并返回自身

HTTP响应会话：结构体--可在System、事件和异步任务间反复转交的同一条HTTP响应
    内部状态：共享引用<互斥锁<HTTP响应会话状态>>--私有
    流缓冲容量：无符号整数--私有
    Clone：公开trait实现
    完整响应：公开方法
        签名：respond(&self, response: HTTP完整响应) -> Result<(), HTTP响应错误>
        行为：
            锁定内部状态
            状态不是等待响应时返回ResponseAlreadyStarted
            取出响应开始发送端并将状态改为正常关闭
            释放锁
            将完整响应转换为Axum Body并交给等待中的Handler
            Handler已经结束时将状态改为异常关闭并返回RequestClosed，否则返回Ok
    开始流式响应：公开方法
        签名：start_stream(&self, head: HTTP响应头) -> Result<(), HTTP响应错误>
        行为：
            创建容量为流缓冲容量的分片通道
            使用分片接收端和head构造Axum流式响应
            锁定内部状态
            状态不是等待响应时返回ResponseAlreadyStarted
            取出响应开始发送端
            将Axum流式响应交给等待中的Handler
            Handler已经结束时将状态改为异常关闭，释放锁并返回RequestClosed
            发送成功时将状态改为正在流式响应，状态持有分片发送端和正常结束标记
            释放锁并返回Ok
    发送分片：公开异步方法
        签名：send_chunk(&self, chunk: Bytes) -> Result<(), HTTP响应错误>
        行为：
            锁定内部状态
            状态为等待响应时返回StreamNotStarted
            状态为正常关闭或异常关闭时返回ResponseClosed
            克隆正在流式响应状态中的分片发送端并释放锁
            异步等待通道容量并发送chunk，以客户端消费速度形成背压
            Axum Body已经结束时将仍处于流式响应的状态改为异常关闭并返回RequestClosed
            发送成功时返回Ok
    正常结束：公开方法
        签名：finish(&self) -> Result<(), HTTP响应错误>
        行为：
            锁定内部状态
            状态为等待响应时返回StreamNotStarted
            状态为正常关闭或异常关闭时返回ResponseClosed
            分片接收端已经被Axum丢弃时将状态改为异常关闭并返回RequestClosed
            将正常结束标记设为true
            将状态改为正常关闭并丢弃状态持有的分片发送端
            已经进入send_chunk的分片完成后，分片通道关闭，Axum正常结束Body
            返回Ok
    异常结束：公开异步方法
        签名：abort(&self, error: HTTP流错误) -> Result<(), HTTP响应错误>
        行为：
            锁定内部状态
            状态为等待响应时返回StreamNotStarted
            状态为正常关闭或异常关闭时返回ResponseClosed
            克隆分片发送端，将状态改为异常关闭并释放锁
            异步等待通道容量并发送error
            Axum收到error后异常结束Body并忽略其后的分片
            Axum Body已经结束时返回RequestClosed，否则返回Ok
    是否已经开始：公开方法
        签名：is_started(&self) -> 布尔值
        行为：状态不是等待响应时返回true
    是否已经关闭：公开方法
        签名：is_closed(&self) -> 布尔值
        行为：状态为正常关闭、异常关闭、响应开始接收端已经丢失或分片接收端已经丢失时返回true

收到HTTP请求：结构体--事件委托路由产生的ECS事件
    请求ID：64位无符号整数--私有，仅用于日志与诊断，响应关联由HTTP响应会话保证
    方法：Method--私有
    URI：Uri--私有
    请求头：HeaderMap--私有
    请求体：Bytes--私有，进入事件队列前已经完整缓冲并执行大小限制
    响应会话：HTTP响应会话--私有
    Event：公开trait实现
    获取请求ID：公开方法
        签名：id(&self) -> 64位无符号整数
        行为：返回请求ID
    获取方法：公开方法
        签名：method(&self) -> Method引用
        行为：返回方法只读引用
    获取URI：公开方法
        签名：uri(&self) -> Uri引用
        行为：返回URI只读引用
    获取请求头：公开方法
        签名：headers(&self) -> HeaderMap引用
        行为：返回请求头只读引用
    获取请求体：公开方法
        签名：body(&self) -> Bytes引用
        行为：返回请求体只读引用
    获取响应会话：公开方法
        签名：response_session(&self) -> HTTP响应会话
        行为：返回同一响应会话的克隆，可交给事件或异步任务
    完整响应：公开方法
        签名：respond(&self, response: HTTP完整响应) -> Result<(), HTTP响应错误>
        行为：调用HTTP响应会话::respond
    开始流式响应：公开方法
        签名：start_stream(&self, head: HTTP响应头) -> Result<(), HTTP响应错误>
        行为：调用HTTP响应会话::start_stream

WebSocket消息：类型别名<axum::extract::ws::Message>--保留文本、二进制、Ping、Pong和Close消息边界

WebSocket连接ID：结构体--服务器为每条WebSocket连接分配的进程内唯一标识
    值：64位无符号整数--私有
    Clone：公开trait实现
    Copy：公开trait实现
    Debug：公开trait实现
    Eq：公开trait实现
    Hash：公开trait实现
    获取值：公开方法
        签名：get(self) -> 64位无符号整数
        行为：返回内部连接ID

WebSocket支流ID：结构体--在同一连接存活期间唯一的应用层流标识
    值：字符串--私有
    Clone：公开trait实现
    Debug：公开trait实现
    Eq：公开trait实现
    Hash：公开trait实现
    使用字符串构造：公开方法
        签名：new(value: impl Into<字符串>) -> Self
        行为：保存字符串并返回自身
    获取字符串：公开方法
        签名：as_str(&self) -> 字符串引用
        行为：返回内部字符串引用

WebSocket支流阶段：枚举
    Start
    Chunk
    End
    Abort
    Clone：公开trait实现
    Copy：公开trait实现
    Debug：公开trait实现
    Eq：公开trait实现

WebSocket消息分类：枚举--分流器只解释路由元数据，不改变原始消息
    Ordinary { message: WebSocket消息 }
    Stream {
        stream_id: WebSocket支流ID,
        phase: WebSocket支流阶段,
        message: WebSocket消息
    }

WebSocket协议错误：枚举
    NonExhaustive：公开属性--允许后续版本增加错误变体
    InvalidEnvelope { message: 字符串 }
    DuplicateStream { connection_id: WebSocket连接ID, stream_id: WebSocket支流ID }
    UnknownStream { connection_id: WebSocket连接ID, stream_id: WebSocket支流ID }
    Debug：公开trait实现
    Display：公开trait实现
        签名：Display for WebSocket协议错误
        行为：输出无效信封或非法支流状态转换的稳定描述
    Error：公开trait实现
        签名：Error for WebSocket协议错误

WebSocket消息分类器：trait--由路由注册者决定消息属于普通事件还是某条接收支流
    分类：公开方法
        签名：classify(&self, message: WebSocket消息) -> Result<WebSocket消息分类, WebSocket协议错误>
        行为：解析消息但保留原始消息，将其分类为普通消息或带ID与阶段的支流消息

默认JSON WebSocket消息分类器：结构体--ServerPlugin提供的默认无状态分类器
    默认：公开trait实现
    WebSocket消息分类器：公开trait实现
        签名：WebSocket消息分类器 for 默认JSON WebSocket消息分类器
        行为：
            文本消息含有mecs_stream对象时读取其中的id与phase
            phase只接受start、chunk、end和abort
            成功时返回携带原始消息的Stream分类
            不含mecs_stream对象的消息返回Ordinary分类
            声明了mecs_stream但结构无效时返回InvalidEnvelope
            非文本消息返回Ordinary分类

WebSocket发送错误：枚举
    NonExhaustive：公开属性--允许后续版本增加错误变体
    BufferFull
    ConnectionClosed
    Debug：公开trait实现
    Display：公开trait实现
        签名：Display for WebSocket发送错误
        行为：输出发送通道已满或连接已经关闭
    Error：公开trait实现
        签名：Error for WebSocket发送错误

WebSocket命名错误：枚举
    NonExhaustive：公开属性--允许后续版本增加错误变体
    ConnectionNotFound { connection_id: WebSocket连接ID }
    NameAlreadyExists { name: 字符串 }
    Debug：公开trait实现
    Display：公开trait实现
        签名：Display for WebSocket命名错误
        行为：输出连接不存在或名称已经被占用
    Error：公开trait实现
        签名：Error for WebSocket命名错误

WebSocket关闭原因：结构体
    关闭码：16位无符号整数--私有
    原因：字符串--私有
    使用关闭码与原因构造：公开方法
        签名：new(code: 16位无符号整数, reason: impl Into<字符串>) -> Self
        行为：保存关闭码与原因并返回自身
    获取关闭码：公开方法
        签名：code(&self) -> 16位无符号整数
        行为：返回关闭码
    获取原因：公开方法
        签名：reason(&self) -> 字符串引用
        行为：返回原因字符串引用

WebSocket发送器：结构体--可在多个事件、System和异步任务间复制与并发使用的连接写入口
    连接ID：WebSocket连接ID--私有
    消息发送端：有界异步发送端<WebSocket消息>--私有
    连接状态：共享引用<共享WebSocket连接状态>--私有
    发送顺序锁：共享引用<异步互斥锁<()>>--私有，使普通消息与Close的状态切换和入队顺序一致
    Clone：公开trait实现
    获取连接ID：公开方法
        签名：connection_id(&self) -> WebSocket连接ID
        行为：返回连接ID
    发送：公开异步方法
        签名：send(&self, message: WebSocket消息) -> Result<(), WebSocket发送错误>
        行为：
            连接已经关闭时返回ConnectionClosed
            异步等待发送通道容量并写入消息
            等待期间连接关闭时返回ConnectionClosed，否则返回Ok
    尝试发送：公开方法
        签名：try_send(&self, message: WebSocket消息) -> Result<(), WebSocket发送错误>
        行为：
            连接已经关闭时返回ConnectionClosed
            通道已满时返回BufferFull
            消息写入通道时返回Ok
    关闭：公开异步方法
        签名：close(&self, reason: WebSocket关闭原因) -> Result<(), WebSocket发送错误>
        行为：只向发送通道写入一次Close消息并将连接标记为正在关闭
    是否已经关闭：公开方法
        签名：is_closed(&self) -> 布尔值
        行为：连接已经关闭、正在关闭或发送通道接收端已经丢失时返回true

WebSocket连接注册表：结构体--ServerPlugin自动维护的全部存活连接发送器
    内部状态：共享引用<读写锁<WebSocket连接注册表状态>>--私有，World与Axum连接任务共享
    Resource：公开trait实现
    Clone：公开trait实现
    根据ID获取发送器：公开方法
        签名：get(&self, connection_id: WebSocket连接ID) -> 可选<WebSocket发送器>
        行为：连接存在时返回发送器克隆，否则返回None
    根据名称获取发送器：公开方法
        签名：get_by_name(&self, name: 字符串引用) -> 可选<WebSocket发送器>
        行为：name为空或不存在时返回None，存在时返回对应发送器克隆
    获取连接名称：公开方法
        签名：name(&self, connection_id: WebSocket连接ID) -> 可选<字符串>
        行为：连接存在且已经具名时返回名称克隆，连接不存在或未具名时返回None
    设置名称：公开方法
        签名：set_name(&self, connection_id: WebSocket连接ID, name: impl Into<字符串>) -> Result<(), WebSocket命名错误>
        行为：
            连接不存在时返回ConnectionNotFound
            name为空时删除旧名称索引并将连接改为不具名
            name已经属于其他连接时返回NameAlreadyExists
            否则原子删除旧名称索引，保存新名称和新名称索引
    获取不具名发送器：公开方法
        签名：unnamed(&self) -> 数组<WebSocket发送器>
        行为：返回当前名称为空的全部存活连接发送器克隆，数组顺序不作保证
    注册连接：crate公开方法
        签名：insert(&self, sender: WebSocket发送器)
        行为：以sender的连接ID写入不具名连接条目；连接ID重复表示内部错误并终止
    移除连接：crate公开方法
        签名：remove(&self, connection_id: WebSocket连接ID)
        行为：原子删除连接条目及其非空名称索引；连接已经不存在时直接返回

WebSocket支流接收错误：枚举
    NonExhaustive：公开属性--允许后续版本增加错误变体
    Aborted
    ConnectionClosed
    Debug：公开trait实现
    Display：公开trait实现
        签名：Display for WebSocket支流接收错误
        行为：输出支流被对端中止或连接已经关闭
    Error：公开trait实现
        签名：Error for WebSocket支流接收错误

WebSocket支流接收器：结构体--一条支流唯一且不可复制的异步消费端
    消息接收端：有界异步接收端<WebSocket消息>--私有
    支流状态：共享引用<共享WebSocket支流状态>--私有
    已报告终止：布尔值--私有
    获取下一条消息：公开异步方法
        签名：recv(&mut self) -> 可选<Result<WebSocket消息, WebSocket支流接收错误>>
        行为：
            通道中存在消息时异步取出并返回Ok
            通道关闭且支流正常结束时返回None
            通道关闭且支流被Abort时只返回一次Aborted，后续返回None
            通道关闭且WebSocket连接断开时只返回一次ConnectionClosed，后续返回None

WebSocket支流接收器句柄：结构体--允许借用事件的System竞争一次支流所有权
    接收器：共享引用<互斥锁<可选<WebSocket支流接收器>>>--私有
    Clone：公开trait实现
    取出接收器：公开方法
        签名：take(&self) -> 可选<WebSocket支流接收器>
        行为：第一次调用取出并返回接收器，后续调用返回None

WebSocket已连接：结构体
    连接ID：WebSocket连接ID--公开
    Event：公开trait实现--仅通知新连接已经注册，发送器通过WebSocket连接注册表查询

收到WebSocket消息：结构体--没有流信封的普通消息事件
    连接ID：WebSocket连接ID--公开
    消息：WebSocket消息--公开
    Event：公开trait实现

WebSocket支流已打开：结构体--只在收到合法Start时产生一次
    连接ID：WebSocket连接ID--公开
    支流ID：WebSocket支流ID--公开
    接收器：WebSocket支流接收器句柄--公开
    Event：公开trait实现

WebSocket已断开：结构体
    连接ID：WebSocket连接ID--公开
    原因：可选<WebSocket关闭原因>--公开
    Event：公开trait实现

WebSocket协议失败：结构体
    连接ID：WebSocket连接ID--公开
    错误：WebSocket协议错误--公开
    Event：公开trait实现

服务器已启动：结构体
    监听地址：SocketAddr--公开
    Event：公开trait实现

服务器失败：结构体
    错误信息：字符串--公开
    Event：公开trait实现

服务器已停止：结构体
    Event：公开trait实现

服务器句柄：结构体--由World持有，可克隆给关闭策略System
    内部状态：共享引用<服务器句柄状态>--私有
    Resource：公开trait实现
    Clone：公开trait实现
    获取监听地址：公开方法
        签名：local_address(&self) -> 可选<SocketAddr>
        行为：返回异步服务绑定成功后写入的实际监听地址
    停止：公开方法
        签名：shutdown(&self)
        行为：只取出并发送一次优雅停止通知，已经请求停止时直接返回

ServerPlugin：结构体
    配置：服务器配置--私有
    默认：公开trait实现
        签名：Default for ServerPlugin
        行为：使用默认服务器配置构造Plugin
    使用配置构造：公开方法
        签名：with_options(options: 服务器配置) -> Self
        行为：使用options构造Plugin
    Plugin：公开trait实现
        签名：build(self, app: App可变引用)
        行为：
            RuntimePlugin不存在时终止并报告RuntimePluginMissing
            AsyncRuntimePlugin不存在时终止并报告AsyncRuntimePluginMissing
            ServerPlugin已经安装时终止并报告ServerPluginAlreadyInstalled
            创建空HTTP路由注册表并作为Resource插入World
            创建空WebSocket连接注册表并作为Resource插入World
            将服务器配置作为Resource插入World
            创建服务器句柄并作为Resource插入World
            将启动服务器System挂到RuntimePlugin::STARTUP

App Server拓展：trait
    添加原生路由：公开方法
        签名：add_http_routes(&mut self, router: Router) -> &mut Self
        行为：
            ServerPlugin不存在时终止并报告ServerPluginMissing
            路由已经冻结时终止并报告RoutesFrozen
            将router合并进HTTP路由注册表
            返回App可变引用
    添加事件委托路由：公开方法
        签名：add_http_event_route(&mut self, method: Method, path: 字符串引用) -> &mut Self
        行为：
            ServerPlugin不存在时终止并报告ServerPluginMissing
            路由已经冻结时终止并报告RoutesFrozen
            相同method与path已经注册时终止并报告EventRouteAlreadyRegistered
            将method与path加入HTTP路由注册表
            返回App可变引用
    添加默认WebSocket事件路由：公开方法
        签名：add_websocket_event_route(&mut self, path: 字符串引用) -> &mut Self
        行为：使用默认JSON WebSocket消息分类器调用add_websocket_event_route_with
    添加自定义WebSocket事件路由：公开泛型方法
        签名：add_websocket_event_route_with<C>(&mut self, path: 字符串引用, classifier: C) -> &mut Self where C: WebSocket消息分类器 + Send + Sync + 'static
        行为：
            ServerPlugin不存在时终止并报告ServerPluginMissing
            路由已经冻结时终止并报告RoutesFrozen
            相同WebSocket path已经注册时终止并报告WebSocketRouteAlreadyRegistered
            对外保留具体分类器类型，存入注册表时擦除分类器类型
            将path与分类器加入HTTP路由注册表
            返回App可变引用
```

私有：
```text
HTTP响应会话状态：枚举
    等待响应 { 响应开始发送端: 可选<oneshot发送端<Axum响应>> }
    正在流式响应 {
        分片发送端: 有界异步发送端<Result<Bytes, HTTP流错误>>,
        正常结束标记: 共享引用<原子布尔值>
    }
    正常关闭
    异常关闭

HTTP流Body：结构体--把内部异步分片通道适配为Axum Body，并监督响应会话是否显式结束
    分片接收端：有界异步接收端<Result<Bytes, HTTP流错误>>
    正常结束标记：共享引用<原子布尔值>--不延长响应会话生命周期
    Body：私有trait实现
        签名：Body for HTTP流Body
        行为：逐个输出分片；通道关闭时仅将正常结束标记视为正常EOF，将会话句柄全部遗失或异常关闭视为Body错误

擦除后的WebSocket消息分类器：类型别名<共享引用<dyn WebSocket消息分类器 + Send + Sync>>

WebSocket事件路由：结构体
    路径：字符串
    消息分类器：擦除后的WebSocket消息分类器

WebSocket连接状态：枚举
    Open
    Closing
    Closed

共享WebSocket连接状态：结构体
    状态：原子8位无符号整数--编码WebSocket连接状态
    读取：私有方法
        签名：load(&self) -> WebSocket连接状态
        行为：原子读取并解码当前连接状态
    尝试开始关闭：私有方法
        签名：begin_close(&self) -> 布尔值
        行为：只允许第一个调用方将Open原子更新为Closing
    标记已经关闭：私有方法
        签名：close(&self)
        行为：将状态原子更新为Closed

WebSocket支流状态：枚举
    Open
    Finished
    Aborted
    ConnectionClosed

共享WebSocket支流状态：结构体
    状态：原子8位无符号整数--编码WebSocket支流状态
    读取：私有方法
        签名：load(&self) -> WebSocket支流状态
        行为：原子读取并解码当前支流状态
    结束：私有方法
        签名：finish(&self)
        行为：将状态标记为Finished
    中止：私有方法
        签名：abort(&self)
        行为：将状态标记为Aborted
    连接断开：私有方法
        签名：disconnect(&self)
        行为：仍为Open时将状态标记为ConnectionClosed

WebSocket支流：结构体--根接收循环持有的一条活动支流
    消息发送端：有界异步发送端<WebSocket消息>
    支流状态：共享引用<共享WebSocket支流状态>

WebSocket支流注册表：类型别名<HashMap<WebSocket支流ID, WebSocket支流>>--每条连接独占，不需要运行时锁

WebSocket连接条目：结构体
    名称：字符串--空字符串表示不具名
    发送器：WebSocket发送器

WebSocket连接注册表状态：结构体
    按ID索引：HashMap<WebSocket连接ID, WebSocket连接条目>
    按名称索引：HashMap<字符串, WebSocket连接ID>--只保存非空名称，保证名称唯一并加速查询

WebSocket连接上下文：结构体--单条WebSocket连接的读取循环和写入循环共享
    连接ID：WebSocket连接ID
    Runtime事件发送器：Runtime事件发送器
    WebSocket发送器：WebSocket发送器
    消息分类器：擦除后的WebSocket消息分类器
    WebSocket通道容量：无符号整数
    Clone：私有trait实现

WebSocket路由状态：结构体--单条WebSocket路由的升级Handler共享
    服务器桥接状态：服务器桥接状态
    消息分类器：擦除后的WebSocket消息分类器
    Clone：私有trait实现

事件委托路由：结构体
    方法：Method
    路径：字符串

HTTP路由注册表：结构体
    原生Router：可选<Router>--启动时取出所有权
    事件委托路由：数组<事件委托路由>
    WebSocket事件路由：数组<WebSocket事件路由>
    已冻结：布尔值
    Resource：crate公开trait实现
    合并原生路由：crate公开方法
        签名：merge(&mut self, router: Router)
        行为：将router合并进原生Router
    添加事件委托路由：crate公开方法
        签名：add_event_route(&mut self, method: Method, path: 字符串)
        行为：检查重复后保存method与path
    添加WebSocket事件路由：crate公开方法
        签名：add_websocket_event_route(&mut self, path: 字符串, classifier: 擦除后的WebSocket消息分类器)
        行为：检查重复后保存path与classifier
    冻结：crate公开方法
        签名：freeze(&mut self, bridge_state: 服务器桥接状态) -> Router
        行为：
            标记注册表已经冻结
            取出原生Router
            为每个事件委托路由创建调用HTTP事件桥接Handler的Axum路由
            为每个WebSocket事件路由创建调用WebSocket升级Handler的Axum路由
            将所有事件委托路由合并进原生Router
            将所有WebSocket事件路由合并进原生Router
            返回最终Router

服务器桥接状态：结构体--由所有HTTP与WebSocket事件委托Handler共享
    Runtime事件发送器：Runtime事件发送器
    WebSocket连接注册表：WebSocket连接注册表
    下一个请求ID：共享原子64位无符号整数
    下一个WebSocket连接ID：共享原子64位无符号整数
    请求体上限：无符号整数
    响应开始超时：Duration
    流缓冲容量：无符号整数
    WebSocket通道容量：无符号整数
    Clone：私有trait实现

服务器句柄状态：结构体
    实际监听地址：写锁<可选<SocketAddr>>
    停止发送端：互斥锁<可选<oneshot发送端<()>>>
```

## 函数

公开：
```text
无
```

私有：
```text
启动服务器System：函数
    签名：start_http_server(world: World可变引用)
    行为：
        从World取得Runtime事件发送器
        从World取得服务器配置、服务器句柄、HTTP路由注册表与WebSocket连接注册表
        使用Runtime事件发送器与WebSocket连接注册表克隆创建服务器桥接状态
        冻结HTTP路由注册表并取得最终Router
        创建优雅停止oneshot通道并将发送端写入服务器句柄
        构造持有配置、Router、句柄、Runtime事件发送器与停止接收端的长期服务Future
        调用WorldAsyncExt::spawn_async_service提交长期服务Future

运行HTTP服务器：异步函数
    签名：run_http_server(options: 服务器配置, router: Router, handle: 服务器句柄, event_sender: Runtime事件发送器, shutdown: oneshot接收端<()>)
    行为：
        异步绑定TCP Listener
        绑定失败时发送服务器失败事件并返回
        将实际监听地址写入服务器句柄
        发送服务器已启动事件
        创建内部优雅停止oneshot通道
        调用axum::serve运行最终Router，并将内部优雅停止接收端传给with_graceful_shutdown
        并发等待Server结束或外部shutdown接收端收到通知
        Server先结束且发生异常时发送服务器失败事件
        外部shutdown先到达时向内部优雅停止发送端发送通知
        在优雅停止超时内继续await Server
        超时后丢弃Server Future，强制取消剩余连接与响应流
        优雅停止期间Server发生异常时发送服务器失败事件
        Server结束后发送服务器已停止事件

HTTP事件桥接Handler：异步函数
    签名：handle_event_request(state: 服务器桥接状态, request: Axum请求) -> Axum响应
    行为：
        按请求体上限完整读取请求Body
        请求体超过上限时立即返回413，不发送ECS事件
        创建响应开始oneshot通道
        使用响应开始发送端和流缓冲容量创建HTTP响应会话
        原子生成请求ID
        使用请求Parts、Bytes与HTTP响应会话构造收到HTTP请求事件
        调用Runtime事件发送器::send_event写入事件并唤醒Runtime
        在响应开始超时范围内await响应开始接收端
        收到完整响应时直接返回完整响应
        收到流式响应时立即返回持有分片接收端的Axum Body
        响应会话在开始响应前全部丢失时返回503
        等待响应开始超时时丢弃响应开始接收端并返回504，响应会话由此变为不可用
        响应开始后不再使用固定总时长限制，直到finish、abort、客户端断连或服务器停止

WebSocket升级Handler：异步函数
    签名：handle_websocket_upgrade(state: WebSocket路由状态, upgrade: WebSocketUpgrade) -> Axum响应
    行为：
        原子生成WebSocket连接ID
        返回WebSocket升级响应
        升级完成后调用run_websocket_connection

运行WebSocket连接：异步函数
    签名：run_websocket_connection(state: WebSocket路由状态, connection_id: WebSocket连接ID, socket: WebSocket)
    行为：
        将socket拆为根接收器与根写入器
        创建容量为WebSocket通道容量的连接写入通道
        创建Open状态的共享WebSocket连接状态
        使用连接ID、写入通道发送端和共享状态构造WebSocket发送器
        构造WebSocket连接上下文
        将发送器以空名称写入WebSocket连接注册表
        发送只携带连接ID的WebSocket已连接通知事件并唤醒Runtime
        启动根接收循环和根写入循环
        任意循环结束时通知另一个循环停止
        等待两个循环完成清理
        将共享连接状态标记为Closed并丢弃写入通道接收端
        从WebSocket连接注册表删除发送器及其名称索引
        发送WebSocket已断开事件并唤醒Runtime

WebSocket根写入循环：异步函数
    签名：write_websocket_messages(context: WebSocket连接上下文, socket_writer: WebSocket写入端, messages: 有界异步接收端<WebSocket消息>, stop: 停止通知) -> 可选<WebSocket关闭原因>
    行为：
        并发等待停止通知或下一条待发送消息
        收到普通消息时写入WebSocket并继续
        收到Close消息时写入WebSocket，返回Close中的关闭原因
        写入失败或消息通道关闭时返回

WebSocket根接收循环：异步函数
    签名：read_websocket_messages(context: WebSocket连接上下文, socket_reader: WebSocket接收端, stop: 停止通知) -> 可选<WebSocket关闭原因>
    行为：
        创建空WebSocket支流注册表
        并发等待停止通知或根接收器的下一条消息
        收到Ping时通过WebSocket发送器回复Pong并继续
        收到Pong时直接继续
        收到文本或二进制消息时调用route_websocket_message
        收到Close消息时保存关闭原因并退出
        根接收器报错、结束或收到停止通知时退出
        将仍然Open的全部支流标记为ConnectionClosed
        清空支流注册表并关闭全部支流通道
        返回关闭原因

分流WebSocket消息：异步函数
    签名：route_websocket_message(context: WebSocket连接上下文, streams: WebSocket支流注册表可变引用, message: WebSocket消息)
    行为：
        使用消息分类器分类message
        分类失败时发送WebSocket协议失败事件并返回
        分类为Ordinary时发送只携带连接ID与消息的收到WebSocket消息事件并返回
        分类为Stream Start时：
            stream_id已经存在则发送DuplicateStream协议失败事件并返回
            创建容量为WebSocket通道容量的支流通道和Open支流状态
            将Start原始消息写入支流通道
            将支流发送端和状态保存到支流注册表
            使用支流接收端构造一次性接收器句柄
            发送携带连接ID、支流ID与唯一接收器句柄的WebSocket支流已打开事件并返回
        分类为Stream Chunk时：
            stream_id不存在则发送UnknownStream协议失败事件并返回
            异步等待对应支流通道容量并写入Chunk原始消息
            接收器已经被丢弃时将支流标记为Aborted，删除支流并返回
        分类为Stream End时：
            stream_id不存在则发送UnknownStream协议失败事件并返回
            从注册表取出对应支流
            异步等待支流通道容量并写入End原始消息
            接收器已经被丢弃时将支流标记为Aborted并返回
            将支流状态标记为Finished并丢弃发送端
            支流接收器读完已排队消息后返回None
        分类为Stream Abort时：
            stream_id不存在则发送UnknownStream协议失败事件并返回
            从注册表取出对应支流
            异步等待支流通道容量并写入Abort原始消息
            接收器已经被丢弃时直接返回
            将支流状态标记为Aborted并丢弃发送端
            支流接收器读完已排队消息后返回一次Aborted
```

## 持有关系

```text
App
└── World
    ├── RuntimeHandle资源
    ├── AsyncRuntimeHandle资源
    ├── HTTP路由注册表资源--第一次STARTUP时冻结
    │   ├── 原生Axum Router
    │   ├── HTTP事件委托路由数组
    │   └── WebSocket事件路由数组
    ├── WebSocket连接注册表资源
    │   └── 共享连接注册表状态
    │       ├── 按ID索引
    │       │   └── WebSocket连接条目
    │       │       ├── 名称--空字符串表示不具名
    │       │       └── WebSocket发送器
    │       └── 按名称索引--仅保存非空唯一名称
    └── 服务器句柄资源
        └── 优雅停止发送端

AsyncRuntime专用线程
└── Axum长期服务Future
    ├── 最终Router
    ├── 服务器桥接状态
    │   ├── Runtime事件发送器
    │   └── WebSocket连接注册表克隆--与World共享同一内部状态
    └── 优雅停止接收端

每个事件委托请求
├── Axum Handler
│   ├── 响应开始接收端--等待respond或start_stream
│   └── 流式Body--start_stream后持有分片接收端
│       └── 正常结束标记--区分显式finish与会话句柄意外遗失
└── 收到HTTP请求事件
    └── HTTP响应会话
        └── 共享响应状态
            ├── 响应开始发送端--响应开始前
            └── 分片发送端--流式响应期间

每条WebSocket连接
├── Axum连接任务
│   ├── WebSocket根接收器
│   │   └── 消息分类器
│   │       ├── 普通消息--发送ECS事件
│   │       └── 支流消息--写入对应支流通道
│   ├── WebSocket根写入器
│   │   └── 连接写入通道接收端
│   └── 支流注册表
│       └── WebSocket支流ID
│           ├── 支流发送端
│           └── 共享支流状态
├── WebSocket连接注册表条目--连接建立时自动写入，断开时自动删除
│   ├── 可选业务名称
│   └── WebSocket发送器
├── ECS事件与异步任务
│   └── 从连接注册表取得的WebSocket发送器克隆--多个生产者共享连接写入通道
└── 每条活动接收支流
    └── WebSocket支流接收器--唯一消费者，可以转交但不可复制
```

## 逻辑

```text
构建服务器：
    app.add_plugin(RuntimePlugin)
        -> app.add_plugin(AsyncRuntimePlugin)
        -> app.add_plugin(ServerPlugin)
        -> 其他Plugin在build中调用add_http_routes或add_http_event_route
        -> app.run

启动服务器：
    -> 第一次tick执行RuntimePlugin::STARTUP
    -> 启动服务器System冻结并合并全部路由
    -> 构造Axum长期服务Future
    -> spawn_async_service将Future提交到AsyncRuntime专用线程
    -> 长期服务不创建pending事件、不关闭阀、不生成自动响应事件

原生Axum路由：
    HTTP请求
        -> Axum匹配其他Plugin注册的Router与Handler
        -> Handler直接执行
        -> Axum直接返回响应

事件委托路由完整响应：
    HTTP请求
        -> Axum匹配ServerPlugin生成的事件桥接Handler
        -> Handler创建HTTP响应会话并发送收到HTTP请求事件
        -> Runtime被唤醒
        -> 用户System读取收到HTTP请求事件
        -> 用户System调用request.respond(response)
        -> HTTP响应会话将完整响应交回Handler并关闭
        -> Axum返回完整响应

事件委托路由流式响应与工具循环：
    前端发送Prompt
        -> Axum Handler创建HTTP响应会话并发送收到HTTP请求事件
        -> Prompt System读取事件
        -> Prompt System调用response.start_stream(head)，Axum立即开始流式响应
        -> Prompt System将HTTP响应会话交给LLM异步任务
        -> LLM异步任务持续await上游流
        -> 每收到一个可输出分片就await response.send_chunk(chunk)
        -> 分片通道已满时等待客户端消费，形成背压
        -> LLM要求调用工具时不结束HTTP响应
        -> LLM异步任务发送工具调用事件，事件携带HTTP响应会话
        -> 工具System读取事件并执行或调度工具
        -> 工具System将工具结果与同一个HTTP响应会话交给下一轮LLM异步任务
        -> 下一轮LLM异步任务继续send_chunk或再次发送工具调用事件
        -> 重复直到LLM不再调用工具
        -> 最终异步任务调用response.finish
        -> 分片通道关闭，Axum正常结束Body

Margatroid计划采用的前端、LLM与同步System数据流--仅记录集成方式，不属于当前ServerPlugin实现：
    前端通过WebSocket发送一条普通Prompt消息
        -> ServerPlugin发送收到WebSocket消息事件
        -> Margatroid同步System解析Prompt并按事件中的连接ID从WebSocket连接注册表取得发送器克隆
        -> 同步System通过send_async_event启动LLM异步任务
    LLM异步任务使用reqwest发起流式HTTP请求
        -> 每收到一个LLM响应分片就追加到本轮累积结果
        -> 同时await WebSocket发送器::send将分片转发给前端
        -> LLM流式HTTP响应结束后返回完整累积结果
        -> AsyncRuntime完成Result响应事件并唤醒Runtime
    Margatroid同步System读取完整LLM响应
        -> 没有tool call时结束本轮处理
        -> 存在tool call时同步处理或调度工具
        -> 将工具结果与同一个WebSocket发送器克隆交给下一轮LLM异步任务
        -> 重复流式转发、累积和同步判断直到tool call循环结束
    reqwest、LLM响应格式、累积结果和tool call循环属于Margatroid业务Plugin，不属于ServerPlugin

流式响应异常：
    业务能够表达的错误
        -> 业务先使用send_chunk发送自身协议的错误分片
        -> 调用finish正常结束HTTP Body
    HTTP流无法继续完成
        -> 调用abort(error)
        -> Axum Body以错误结束
    客户端断开连接
        -> Axum丢弃分片接收端
        -> 后续send_chunk返回RequestClosed
        -> 异步任务停止后续LLM或工具循环
    所有HTTP响应会话句柄在没有finish或abort时被丢弃
        -> 分片发送端随内部状态销毁
        -> HTTP流Body发现通道关闭但正常结束标记仍为false
        -> Axum Body以响应未完成错误结束

WebSocket普通消息：
    前端建立WebSocket连接
        -> Axum持有根接收器与根写入器
        -> 创建连接写入通道和可克隆WebSocket发送器
        -> ServerPlugin先将发送器以空名称写入WebSocket连接注册表
        -> 发送只携带连接ID的WebSocket已连接通知事件
        -> 用户System可以根据连接ID设置唯一业务名称，也可以保持不具名
    前端发送不含流信封的消息
        -> 根接收循环将消息分类为Ordinary
        -> 发送携带连接ID与消息的收到WebSocket消息事件
        -> 同步System处理消息
        -> System根据连接ID从WebSocket连接注册表取得发送器
        -> System可以使用发送器try_send立即响应
        -> System也可以将发送器克隆交给异步任务
        -> 异步任务await sender.send发送一条或多条消息
        -> 根写入循环串行写入WebSocket
    前端断开WebSocket连接
        -> ServerPlugin先从WebSocket连接注册表删除连接及其名称索引
        -> 发送WebSocket已断开通知事件
        -> 通知只供取消业务任务、更新在线状态和记录指标，不负责清理发送器

WebSocket发送器命名与查询：
    新连接默认名称为空，属于不具名发送器
    用户System收到连接通知后可以按连接ID调用set_name
    设置非空名称时在写锁内检查全局唯一性并同步更新ID索引与名称索引
    设置空名称时删除旧名称索引，使连接重新成为不具名发送器
    get按连接ID查询一个发送器，get_by_name按非空唯一名称查询一个发送器
    unnamed返回当前全部不具名发送器克隆，顺序不作保证
    连接在通知被System处理前已经断开时查询可能返回None，用户直接忽略即可

WebSocket默认流信封：
    JSON形状--{"mecs_stream":{"id":"request-1","phase":"chunk"},"payload":任意JSON值}
    普通消息--不含mecs_stream对象
    Start--mecs_stream包含id与phase=start
    Chunk--mecs_stream包含同一id与phase=chunk
    End--mecs_stream包含同一id与phase=end
    Abort--mecs_stream包含同一id与phase=abort
    完整支流键--WebSocket连接ID + WebSocket支流ID
    默认分类器只解析路由所需的mecs_stream，支流接收器仍收到完整原始消息
    二进制流或其他信封格式通过add_websocket_event_route_with注册自定义分类器

WebSocket流式接收：
    前端发送Start
        -> 根接收循环创建该stream_id的有界支流通道
        -> Start原始消息进入支流通道
        -> 只发送一次WebSocket支流已打开事件
        -> 同步System从事件句柄take唯一支流接收器
        -> System调用send_async_event将接收器移动进异步累积任务
    前端持续发送Chunk
        -> 根接收循环按connection_id与stream_id直接分流
        -> Chunk原始消息进入对应支流通道
        -> Chunk不进入ECS事件队列
        -> 异步任务持续recv并累积或即时处理
    前端发送End
        -> End原始消息进入支流通道
        -> 根接收循环删除支流发送端并标记Finished
        -> 异步任务读完已排队消息后得到None
        -> 异步任务返回累积结果
        -> AsyncRuntime完成对应的Result响应事件并唤醒Runtime
        -> 用户同步System读取累积结果事件
        -> 支流接收器被释放
    前端发送Abort或连接断开
        -> 根接收循环标记支流Aborted或ConnectionClosed
        -> 关闭支流通道
        -> 异步任务从recv得到对应错误

WebSocket并发与背压：
    多个WebSocket发送器并发写入同一个有界连接通道
        -> 单条消息边界保持完整
        -> 根写入循环按通道实际接收顺序串行发送
        -> 并发生产者之间的先后顺序不作保证，业务需要时自行携带sequence
    一条支流通道已满
        -> 根接收循环等待该支流消费者释放容量
        -> 整条WebSocket暂时停止读取，形成连接级背压
        -> 默认不丢消息并保持每条支流顺序

ServerPlugin边界：
    请求Body在进入ECS事件前完整缓冲，不支持通过事件逐块读取请求Body
    响应既可以一次完整返回，也可以通过HTTP响应会话多次流式发送
    HTTP响应会话可以在System、事件与异步任务之间不限次数转交
    每条流必须显式调用finish或abort，丢弃全部会话句柄不代表成功完成
    LLM流式响应和tool call循环属于事件委托路由的主要使用场景
    WebSocket普通消息通过ECS事件同步处理，带Start与End的支流消息通过异步通道累积
    WebSocket流分片不进入ECS事件队列，事件队列只接收连接、普通消息和支流打开等状态事件
    流式HTTP上传和依赖原生Axum提取器或中间件的协议使用原生Axum路由
    文件下载复用HTTP响应会话，文件存储与落盘不属于ServerPlugin
    ServerPlugin只负责网络服务生命周期、协议传输句柄与ECS桥接，不解释prompt、token、tool call或业务协议
    当前内置HTTP和基于HTTP Upgrade的WebSocket，未来协议通过独立模块或扩展接入
    ServerPlugin名称不代表必须将所有网络协议集中实现到同一个crate
```
