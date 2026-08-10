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

# ServerPlugin

设计原则：同步处理，异步累积；事件传递状态，通道传递连续数据。

crate名称：server_plugin。

## 类型

公开：
```text
HttpResponse：HTTP完整响应，公开类型别名--等于Response<Bytes>

HttpResponseHead：HTTP响应头，公开结构体--流式响应开始时一次性确定状态码和响应头
    status: StatusCode--状态码，私有，默认200
    headers: HeaderMap--响应头，私有
    with_status(mut self, status: StatusCode) -> Self
        设置状态码：公开方法，status替换当前状态码
        行为：保存status并返回自身
    with_header(mut self, name: HeaderName, value: HeaderValue) -> Self
        设置响应头：公开方法，name和value指定要写入的响应头
        行为：写入响应头并返回自身
    impl Default for HttpResponseHead
        Default：公开trait实现
        default() -> Self
            构造默认响应头：返回200且没有额外响应头的HttpResponseHead

HttpResponseError：HTTP响应错误，公开枚举--描述响应会话当前状态不允许的操作
    ResponseAlreadyStarted
    StreamNotStarted
    ResponseClosed
    RequestClosed
    impl fmt::Display for HttpResponseError
        Display：公开trait实现，输出稳定错误描述
    impl std::error::Error for HttpResponseError
        Error：公开trait实现

HttpStreamError：HTTP流错误，公开结构体--无法继续完成响应流时交给Axum Body
    message: String--错误信息，私有
    new(message: impl Into<String>) -> Self
        使用消息构造：公开关联函数，message描述流失败原因
        行为：保存错误信息并返回自身
    abandoned() -> Self
        构造遗弃错误：crate公开关联函数
        行为：返回说明响应流未调用finish或abort的固定错误
    impl fmt::Display for HttpStreamError
        Display：公开trait实现，输出message
    impl std::error::Error for HttpStreamError
        Error：公开trait实现

ServerError：服务器错误，公开枚举--描述Plugin依赖、路由注册和配置错误
    RuntimePluginMissing
    AsyncRuntimePluginMissing
    ServerPluginAlreadyInstalled
    ServerPluginMissing
    RoutesFrozen
    EventRouteAlreadyRegistered { method: Method, path: String }
    WebSocketRouteAlreadyRegistered { path: String }
    UnsupportedMethod { method: Method }
    InvalidBodyLimit { limit: usize }
    InvalidResponseStartTimeout { timeout: Duration }
    InvalidStreamBufferCapacity { capacity: usize }
    InvalidWebSocketBufferCapacity { capacity: usize }
    InvalidShutdownTimeout { timeout: Duration }
    panic(self) -> !
        终止：crate公开方法，消费当前ServerError
        行为：使用自身Display描述触发panic
    impl fmt::Display for ServerError
        Display：公开trait实现，输出包含上下文的稳定错误描述
    impl std::error::Error for ServerError
        Error：公开trait实现

ServerOptions：服务器配置，公开结构体--配置监听、HTTP、WebSocket和停止边界
    bind_address: SocketAddr--监听地址，crate公开，默认127.0.0.1:3939
    body_limit: usize--请求体上限，crate公开
    response_start_timeout: Duration--响应开始超时，crate公开
    stream_buffer_capacity: usize--HTTP流缓冲容量，crate公开
    websocket_buffer_capacity: usize--WebSocket通道容量，crate公开
    shutdown_timeout: Duration--优雅停止超时，crate公开
    DEFAULT_PORT: u16--默认端口3939，公开关联常量
    bind(address: SocketAddr) -> Self
        使用地址构造：公开关联函数，address替换默认监听地址
        行为：保留其他默认配置并返回自身
    with_bind(mut self, address: SocketAddr) -> Self
        设置监听地址：公开方法，address替换当前地址
        行为：保存address并返回自身
    with_body_limit(mut self, limit: usize) -> Self
        设置请求体上限：公开方法，limit是最大字节数
        行为：limit为0时报告InvalidBodyLimit，否则保存并返回自身
    with_response_start_timeout(mut self, timeout: Duration) -> Self
        设置响应开始超时：公开方法
        行为：timeout为0时报告InvalidResponseStartTimeout，否则保存并返回自身
    with_stream_buffer_capacity(mut self, capacity: usize) -> Self
        设置HTTP流缓冲容量：公开方法
        行为：capacity为0时报告InvalidStreamBufferCapacity，否则保存并返回自身
    with_websocket_buffer_capacity(mut self, capacity: usize) -> Self
        设置WebSocket通道容量：公开方法
        行为：capacity为0时报告InvalidWebSocketBufferCapacity，否则保存并返回自身
    with_shutdown_timeout(mut self, timeout: Duration) -> Self
        设置停止超时：公开方法
        行为：timeout为0时报告InvalidShutdownTimeout，否则保存并返回自身
    bind_address(&self) -> SocketAddr
        获取监听地址：公开方法，返回bind_address
    body_limit(&self) -> usize
        获取请求体上限：公开方法，返回body_limit
    response_start_timeout(&self) -> Duration
        获取响应开始超时：公开方法，返回response_start_timeout
    stream_buffer_capacity(&self) -> usize
        获取HTTP流缓冲容量：公开方法，返回stream_buffer_capacity
    websocket_buffer_capacity(&self) -> usize
        获取WebSocket通道容量：公开方法，返回websocket_buffer_capacity
    shutdown_timeout(&self) -> Duration
        获取停止超时：公开方法，返回shutdown_timeout
    impl Default for ServerOptions
        Default：公开trait实现，构造安全默认配置
    impl Resource for ServerOptions
        Resource：公开trait实现

HttpResponseSession：HTTP响应会话，公开结构体--可在System、事件和异步任务间转交的同一条HTTP响应
    state: Arc<Mutex<HttpResponseState>>--响应状态，私有
    stream_buffer_capacity: usize--流缓冲容量，私有
    new(response_sender: oneshot::Sender<Response<Body>>, stream_buffer_capacity: usize) -> Self
        构造会话：crate公开关联函数，response_sender交回开始响应，stream_buffer_capacity限制分片通道
        行为：构造Waiting状态并保存流缓冲容量
    respond(&self, response: HttpResponse) -> Result<(), HttpResponseError>
        完整响应：公开方法，response是一次返回的完整响应
        行为：仅Waiting状态可用；将响应交给Handler并正常关闭，Handler已结束时返回RequestClosed
    start_stream(&self, head: HttpResponseHead) -> Result<(), HttpResponseError>
        开始流式响应：公开方法，head确定响应状态和头
        行为：仅Waiting状态可用；创建有界分片通道，将Body交给Handler并进入Streaming
    send_chunk(&self, chunk: Bytes) -> Result<(), HttpResponseError>
        发送分片：公开异步方法，chunk是一个HTTP Body分片
        行为：等待通道容量并发送，以客户端消费速度形成背压
    finish(&self) -> Result<(), HttpResponseError>
        正常结束：公开方法
        行为：仅Streaming状态可用；设置正常结束标记并关闭通道
    abort(&self, error: HttpStreamError) -> Result<(), HttpResponseError>
        异常结束：公开异步方法，error作为Body终止错误
        行为：仅Streaming状态可用；发送错误并将会话标记为异常关闭
    is_started(&self) -> bool
        检查已开始：公开方法，非Waiting状态返回true
    is_closed(&self) -> bool
        检查已关闭：公开方法，会话或对端已关闭时返回true

HttpRequestReceived：收到HTTP请求，公开结构体--事件委托路由将完整请求包装为该事件
    id: u64--请求ID，私有
    method: Method--HTTP方法，私有
    uri: Uri--URI，私有
    headers: HeaderMap--请求头，私有
    body: Bytes--已完整缓冲的请求体，私有
    response: HttpResponseSession--响应会话，私有
    new(id: u64, method: Method, uri: Uri, headers: HeaderMap, body: Bytes, response: HttpResponseSession) -> Self
        构造请求事件：crate公开关联函数，封装已分解且已缓冲的HTTP请求
    id(&self) -> u64
        获取ID：公开方法，返回请求ID
    method(&self) -> &Method
        获取方法：公开方法，返回HTTP方法引用
    uri(&self) -> &Uri
        获取URI：公开方法，返回URI引用
    headers(&self) -> &HeaderMap
        获取请求头：公开方法，返回HeaderMap引用
    body(&self) -> &Bytes
        获取请求体：公开方法，返回Bytes引用
    response_session(&self) -> HttpResponseSession
        取得响应会话：公开方法，返回会话克隆
    respond(&self, response: HttpResponse) -> Result<(), HttpResponseError>
        完整响应：公开便捷方法，转发给HttpResponseSession::respond
    start_stream(&self, head: HttpResponseHead) -> Result<(), HttpResponseError>
        开始流式响应：公开便捷方法，转发给HttpResponseSession::start_stream
    impl Event for HttpRequestReceived
        Event：公开trait实现

ServerStarted：服务器已启动，公开结构体
    address: SocketAddr--实际监听地址，公开
    impl Event for ServerStarted
        Event：公开trait实现

ServerFailed：服务器失败，公开结构体
    message: String--错误信息，公开
    impl Event for ServerFailed
        Event：公开trait实现

ServerStopped：服务器已停止，公开单元结构体
    impl Event for ServerStopped
        Event：公开trait实现

WebSocketMessage：WebSocket消息，公开类型别名--等于axum::extract::ws::Message

WebSocketConnectionId：WebSocket连接ID，公开元组结构体
    0: u64--连接ID，私有
    new(value: u64) -> Self
        构造ID：crate公开关联函数，封装value
    get(self) -> u64
        获取ID：公开方法，返回内部u64

RegisterConnection：连接注册事件，公开结构体--声明连接的客户端类型，供连接元数据插件消费
    id: String--客户端生成的注册请求ID
    connection_id: WebSocketConnectionId--收到注册请求的连接
    client_type: String--客户端声明的类型，例如webui或cli
    impl Event for RegisterConnection
        Event：公开trait实现

WebSocketStreamId：WebSocket支流ID，公开元组结构体
    0: String--支流ID，私有
    new(value: impl Into<String>) -> Self
        构造ID：公开关联函数，value提供字符串ID
        行为：将value转为String并封装
    as_str(&self) -> &str
        获取字符串：公开方法，返回支流ID引用

WebSocketStreamPhase：WebSocket支流阶段，公开枚举
    Start
    Chunk
    End
    Abort

WebSocketMessageClassification：WebSocket消息分类，公开枚举--区分普通消息和带支流信封的消息
    Ordinary { message: WebSocketMessage }
    Stream { stream_id: WebSocketStreamId, phase: WebSocketStreamPhase, message: WebSocketMessage }

WebSocketProtocolError：WebSocket协议错误，公开枚举
    InvalidEnvelope { message: String }
    DuplicateStream { connection_id: WebSocketConnectionId, stream_id: WebSocketStreamId }
    UnknownStream { connection_id: WebSocketConnectionId, stream_id: WebSocketStreamId }
    impl fmt::Display for WebSocketProtocolError
        Display：公开trait实现，输出信封或支流状态错误
    impl std::error::Error for WebSocketProtocolError
        Error：公开trait实现

WebSocketMessageClassifier：WebSocket消息分类器，公开trait--将一条原始消息分为普通消息或支流消息
    继承：Send + Sync + 'static
    classify(&self, message: WebSocketMessage) -> Result<WebSocketMessageClassification, WebSocketProtocolError>
        分类：公开方法，message是待分类的原始消息

JsonWebSocketMessageClassifier：JSON WebSocket消息分类器，公开单元结构体--实现默认mecs_stream信封
    impl WebSocketMessageClassifier for JsonWebSocketMessageClassifier
        WebSocketMessageClassifier：公开trait实现
        classify(&self, message: WebSocketMessage) -> Result<WebSocketMessageClassification, WebSocketProtocolError>
            分类：非文本、非JSON或不含mecs_stream时返回Ordinary，否则验证id和phase并返回Stream

WebSocketCloseReason：WebSocket关闭原因，公开结构体
    code: u16--关闭码，私有
    reason: String--原因，私有
    new(code: u16, reason: impl Into<String>) -> Self
        构造：公开关联函数，code和reason组成关闭原因
    code(&self) -> u16
        获取关闭码：公开方法
    reason(&self) -> &str
        获取原因：公开方法
    from_frame(frame: CloseFrame) -> Self
        从CloseFrame构造：crate公开关联函数
    into_frame(self) -> CloseFrame
        转换为CloseFrame：私有方法，消费自身并转移reason

WebSocketSendError：WebSocket发送错误，公开枚举
    BufferFull
    ConnectionClosed
    impl fmt::Display for WebSocketSendError
        Display：公开trait实现，输出缓冲已满或连接已关闭
    impl std::error::Error for WebSocketSendError
        Error：公开trait实现

WebSocketSender：WebSocket发送器，公开结构体--可在事件、System和异步任务间克隆的连接写入口
    connection_id: WebSocketConnectionId--连接ID，私有
    sender: mpsc::Sender<WebSocketMessage>--消息发送端，私有
    state: Arc<SharedConnectionState>--连接状态，私有
    send_lock: Arc<tokio::sync::Mutex<()>>--发送顺序锁，私有
    new(connection_id: WebSocketConnectionId, sender: mpsc::Sender<WebSocketMessage>, state: Arc<SharedConnectionState>) -> Self
        构造发送器：crate公开关联函数，保存连接标识、通道和共享状态并创建发送锁
    connection_id(&self) -> WebSocketConnectionId
        获取连接ID：公开方法
    send(&self, message: WebSocketMessage) -> Result<(), WebSocketSendError>
        发送：公开异步方法，等待有界通道容量并保持单个发送器的入队顺序
    try_send(&self, message: WebSocketMessage) -> Result<(), WebSocketSendError>
        尝试发送：公开方法，不等待容量，忙或已满时返回BufferFull
    close(&self, reason: WebSocketCloseReason) -> Result<(), WebSocketSendError>
        关闭：公开异步方法，只允许第一个调用方将Close消息入队
    is_closed(&self) -> bool
        检查关闭：公开方法，非Open或接收端已丢失时返回true
    impl PartialEq for WebSocketSender
        PartialEq：公开trait实现，仅比较connection_id
    impl Eq for WebSocketSender
        Eq：公开trait实现
    impl Hash for WebSocketSender
        Hash：公开trait实现，仅哈希connection_id

WebSocketMessageSender：WebSocket消息发送终端，公开结构体--持有已解析的连接发送器和可直接入队的WebSocket消息，不是Event
    senders: Vec<WebSocketSender>--本次消息的固定接收连接集合，私有
    message: WebSocketMessage--已序列化或已构造的WebSocket帧，私有
    new(senders: Vec<WebSocketSender>, message: WebSocketMessage) -> Self
        构造发送终端：公开关联函数，保存固定发送器集合和消息
    send(self) -> Future<Output = Vec<(WebSocketConnectionId, Result<(), WebSocketSendError>)>>
        异步发送：公开方法，按senders顺序逐个await WebSocketSender::send并汇总结果
    try_send(self) -> Vec<(WebSocketConnectionId, Result<(), WebSocketSendError>)>
        尝试发送：公开方法，按senders顺序逐个调用WebSocketSender::try_send并汇总结果

WebSocketNameError：WebSocket命名错误，公开枚举
    ConnectionNotFound { connection_id: WebSocketConnectionId }
    NameAlreadyExists { name: String }
    impl fmt::Display for WebSocketNameError
        Display：公开trait实现，输出缺失连接或名称重复
    impl std::error::Error for WebSocketNameError
        Error：公开trait实现

WebSocketConnections：WebSocket连接注册表，公开结构体--ServerPlugin自动维护全部存活连接发送器
    state: Arc<RwLock<WebSocketConnectionsState>>--连接索引，私有
    new() -> Self
        构造注册表：crate公开关联函数，创建空的共享连接索引
    get(&self, connection_id: WebSocketConnectionId) -> Option<WebSocketSender>
        按ID查询：公开方法，返回发送器克隆
    get_by_name(&self, name: &str) -> Option<WebSocketSender>
        按名称查询：公开方法，空名称或不存在时返回None
    name(&self, connection_id: WebSocketConnectionId) -> Option<String>
        查询名称：公开方法，连接未具名时返回None
    set_name(&self, connection_id: WebSocketConnectionId, name: impl Into<String>) -> Result<(), WebSocketNameError>
        设置名称：公开方法，名称全局唯一，空名称将连接改为不具名
    connection_type(&self, connection_id: WebSocketConnectionId) -> Option<String>
        查询连接类型：公开方法，连接不存在或尚未设置类型时返回None
    set_connection_type(&self, connection_id: WebSocketConnectionId, connection_type: impl Into<String>) -> bool
        设置连接类型：公开方法，原子更新连接条目和类型索引，连接不存在时返回false，空类型清除类型
    get_all(&self) -> Vec<WebSocketSender>
        查询全部发送器：公开方法，返回全部存活连接的发送器克隆，顺序不作保证
    get_by_type(&self, connection_type: &str) -> Vec<WebSocketSender>
        按类型查询发送器：公开方法，返回类型相同的全部存活连接发送器克隆，空类型返回空数组
    get_unnamed(&self) -> Vec<WebSocketSender>
        查询不具名发送器：公开方法，返回顺序不作保证的克隆数组
    insert(&self, sender: WebSocketSender)
        注册连接：crate公开方法，以空名称和空连接类型写入sender，ID重复时panic
    remove(&self, connection_id: WebSocketConnectionId)
        移除连接：crate公开方法，原子删除ID条目、非空名称索引和非空连接类型索引
    impl Resource for WebSocketConnections
        Resource：公开trait实现

WebSocketStreamReceiveError：WebSocket支流接收错误，公开枚举
    Aborted
    ConnectionClosed
    impl fmt::Display for WebSocketStreamReceiveError
        Display：公开trait实现
    impl std::error::Error for WebSocketStreamReceiveError
        Error：公开trait实现

WebSocketStreamReceiver：WebSocket支流接收器，公开结构体--一条支流唯一且不可克隆的异步消费端
    receiver: mpsc::Receiver<WebSocketMessage>--消息接收端，私有
    state: Arc<SharedStreamState>--支流状态，私有
    terminal_reported: bool--是否已报告终止错误，私有
    new(receiver: mpsc::Receiver<WebSocketMessage>, state: Arc<SharedStreamState>) -> Self
        构造接收器：crate公开关联函数，保存通道和支流状态
    recv(&mut self) -> Option<Result<WebSocketMessage, WebSocketStreamReceiveError>>
        接收：公开异步方法，正常结束返回None，Abort或连接断开只报告一次错误

WebSocketStreamReceiverHandle：WebSocket支流接收器句柄，公开结构体--允许多个System借用事件时竞争一次接收器所有权
    receiver: Arc<Mutex<Option<WebSocketStreamReceiver>>>--仅能取出一次的接收器，私有
    new(receiver: WebSocketStreamReceiver) -> Self
        构造句柄：crate公开关联函数，将receiver包装为可共享的一次所有权
    take(&self) -> Option<WebSocketStreamReceiver>
        取出：公开方法，第一次返回接收器，后续返回None

WebSocketConnected：WebSocket已连接，公开结构体--只通知新连接已写入注册表
    connection_id: WebSocketConnectionId--连接ID，公开
    impl Event for WebSocketConnected
        Event：公开trait实现

WebSocketMessageReceived：收到WebSocket普通消息，公开结构体
    connection_id: WebSocketConnectionId--连接ID，公开
    message: WebSocketMessage--原始消息，公开
    impl Event for WebSocketMessageReceived
        Event：公开trait实现

WebSocketStreamOpened：WebSocket支流已打开，公开结构体--只在收到合法Start时产生一次
    connection_id: WebSocketConnectionId--连接ID，公开
    stream_id: WebSocketStreamId--支流ID，公开
    receiver: WebSocketStreamReceiverHandle--接收器句柄，公开
    impl Event for WebSocketStreamOpened
        Event：公开trait实现

WebSocketDisconnected：WebSocket已断开，公开结构体
    connection_id: WebSocketConnectionId--连接ID，公开
    reason: Option<WebSocketCloseReason>--关闭原因，公开
    impl Event for WebSocketDisconnected
        Event：公开trait实现

WebSocketProtocolFailed：WebSocket协议失败，公开结构体
    connection_id: WebSocketConnectionId--连接ID，公开
    error: WebSocketProtocolError--协议错误，公开
    impl Event for WebSocketProtocolFailed
        Event：公开trait实现

ServerHandle：服务器句柄，公开结构体--查询服务器状态或请求优雅停止
    state: Arc<ServerHandleState>--共享句柄状态，私有
    new() -> Self
        构造句柄：crate公开关联函数，创建未监听且没有停止发送端的共享状态
    set_local_address(&self, address: SocketAddr)
        设置监听地址：crate公开方法，在绑定成功后保存address
    set_shutdown_sender(&self, sender: oneshot::Sender<()>)
        设置停止发送端：crate公开方法，保存sender
    mark_stopped(&self)
        标记已停止：crate公开方法，清空监听地址和停止发送端
    local_address(&self) -> Option<SocketAddr>
        获取监听地址：公开方法，未启动或已停止时返回None
    is_running(&self) -> bool
        检查运行：公开方法，local_address存在时返回true
    shutdown(&self)
        停止：公开方法，只取出并发送一次优雅停止通知
    impl Default for ServerHandle
        Default：公开trait实现，构造未运行句柄
    impl Resource for ServerHandle
        Resource：公开trait实现

ServerPlugin：服务器Plugin，公开结构体
    options: ServerOptions--服务器配置，私有
    bind(address: impl ToSocketAddrs) -> Self
        使用地址构造：公开关联函数，解析address并使用首个SocketAddr
    with_options(options: ServerOptions) -> Self
        使用配置构造：公开关联函数，保存options
    with_body_limit(mut self, limit: usize) -> Self
        设置请求体上限：公开便捷方法，转发给ServerOptions
    with_response_start_timeout(mut self, timeout: Duration) -> Self
        设置响应开始超时：公开便捷方法，转发给ServerOptions
    with_stream_buffer_capacity(mut self, capacity: usize) -> Self
        设置HTTP流缓冲容量：公开便捷方法，转发给ServerOptions
    with_websocket_buffer_capacity(mut self, capacity: usize) -> Self
        设置WebSocket通道容量：公开便捷方法，转发给ServerOptions
    with_shutdown_timeout(mut self, timeout: Duration) -> Self
        设置停止超时：公开便捷方法，转发给ServerOptions
    impl Default for ServerPlugin
        Default：公开trait实现，使用默认ServerOptions
    impl Plugin for ServerPlugin
        Plugin：公开trait实现
        build(self, app: &mut App)
            构建Plugin：检查RuntimePlugin和AsyncRuntimePlugin，插入路由、连接、配置和句柄资源，将start_server挂到STARTUP

AppServerExt：App服务器扩展，公开trait--在ServerPlugin启动前注册原生Axum路由和ECS委托路由
    add_http_routes(&mut self, router: Router) -> &mut Self
        添加原生路由：公开方法，router合并进RouteRegistry
    add_http_event_route(&mut self, method: Method, path: &str) -> &mut Self
        添加HTTP事件委托路由：公开方法，method和path唯一标识路由
    add_websocket_event_route(&mut self, path: &str) -> &mut Self
        添加默认WebSocket路由：公开方法，使用JsonWebSocketMessageClassifier
    add_websocket_event_route_with<C>(&mut self, path: &str, classifier: C) -> &mut Self
        添加自定义WebSocket路由：公开泛型方法，classifier决定消息分流
        约束：C: WebSocketMessageClassifier + Send + Sync + 'static
    impl AppServerExt for App
        AppServerExt for App：公开trait实现，ServerPlugin缺失、路由已冻结或路由重复时报告ServerError
```

crate公开：
```text
ErasedWebSocketClassifier：擦除WebSocket分类器，crate公开类型别名--等于Arc<dyn WebSocketMessageClassifier + Send + Sync + 'static>

EventRoute：HTTP事件委托路由，crate公开结构体
    method: Method--HTTP方法，crate公开
    path: String--路径，crate公开

WebSocketRoute：WebSocket事件路由，crate公开结构体
    path: String--路径，crate公开
    classifier: ErasedWebSocketClassifier--消息分类器，crate公开

RouteRegistry：路由注册表，crate公开结构体--允许通过共享引用配置，STARTUP时一次性冻结
    state: Mutex<RouteRegistryState>--路由状态，私有
    new() -> Self
        构造：crate公开关联函数，创建空Router和空委托路由数组
    merge(&self, router: Router)
        合并原生路由：crate公开方法，冻结后报告RoutesFrozen
    add_event_route(&self, method: Method, path: String)
        添加HTTP委托路由：crate公开方法，检查method和path重复
    add_websocket_route(&self, path: String, classifier: ErasedWebSocketClassifier)
        添加WebSocket路由：crate公开方法，检查path重复
    freeze(&self) -> (Router, Vec<EventRoute>, Vec<WebSocketRoute>)
        冻结：crate公开方法
        行为：原子标记冻结，取出Router和两类路由的所有权；重复冻结时报告RoutesFrozen
    impl Resource for RouteRegistry
        Resource：crate公开trait实现

ConnectionState：WebSocket连接状态，crate公开枚举
    Open
    Closing
    Closed

SharedConnectionState：共享WebSocket连接状态，crate公开元组结构体
    0: AtomicU8--编码后的ConnectionState，私有
    open() -> Self
        构造Open状态：crate公开关联函数
    load(&self) -> ConnectionState
        读取：crate公开方法，原子读取并解码
    begin_close(&self) -> bool
        尝试开始关闭：私有方法，仅第一个调用方能将Open原子更新为Closing
    close(&self)
        标记已关闭：crate公开方法，原子写入Closed

StreamState：WebSocket支流状态，crate公开枚举
    Open
    Finished
    Aborted
    ConnectionClosed

SharedStreamState：共享WebSocket支流状态，crate公开元组结构体
    0: AtomicU8--编码后的StreamState，私有
    open() -> Self
        构造Open状态：crate公开关联函数
    load(&self) -> StreamState
        读取：crate公开方法
    finish(&self)
        正常结束：crate公开方法
    abort(&self)
        中止：crate公开方法
    disconnect(&self)
        连接断开：crate公开方法，仅将Open原子更新为ConnectionClosed

WebSocketStream：WebSocket活动支流，crate公开结构体--根接收循环持有
    sender: mpsc::Sender<WebSocketMessage>--支流发送端，crate公开
    state: Arc<SharedStreamState>--支流状态，crate公开
```

私有：
```text
HttpResponseState：HTTP响应状态，私有枚举
    Waiting(Option<oneshot::Sender<Response<Body>>>)
    Streaming { sender: mpsc::Sender<Result<Bytes, HttpStreamError>>, finished: Arc<AtomicBool> }
    ClosedNormally
    ClosedWithError

HttpResponseBodyStream：HTTP Body流，私有结构体--将分片通道适配为Axum Body并监督显式结束
    receiver: mpsc::Receiver<Result<Bytes, HttpStreamError>>--分片接收端
    finished: Arc<AtomicBool>--正常结束标记
    terminal_reported: bool--是否已输出终止错误
    impl Stream for HttpResponseBodyStream
        Stream：私有trait实现，输出分片；通道意外关闭时输出HttpStreamError::abandoned
        Item = Result<Bytes, HttpStreamError>
            分片类型：成功为Body字节，失败为终止流的HttpStreamError

RouteRegistryState：路由注册表状态，私有结构体
    native_router: Option<Router>--原生Router，冻结时取出
    event_routes: Vec<EventRoute>--HTTP委托路由
    websocket_routes: Vec<WebSocketRoute>--WebSocket路由
    frozen: bool--是否已冻结

ServerHandleState：服务器句柄状态，私有结构体
    local_address: RwLock<Option<SocketAddr>>--实际监听地址
    shutdown_sender: Mutex<Option<oneshot::Sender<()>>>--优雅停止发送端
    impl Drop for ServerHandleState
        Drop：私有trait实现，最后一个句柄释放时发送停止通知

WebSocketConnectionEntry：WebSocket连接条目，私有结构体
    name: String--名称，空字符串表示不具名
    connection_type: String--连接类型，空字符串表示尚未注册类型
    sender: WebSocketSender--发送器

WebSocketConnectionsState：WebSocket连接索引，私有结构体
    by_id: HashMap<WebSocketConnectionId, WebSocketConnectionEntry>--ID索引
    by_name: HashMap<String, WebSocketConnectionId>--非空唯一名称索引
    by_type: HashMap<String, HashSet<WebSocketConnectionId>>--非空连接类型到连接ID集合的索引

ServerBridgeState：服务器桥接状态，私有结构体--HTTP和WebSocket Handler共享
    event_sender: RuntimeEventSender--Runtime事件发送器
    websocket_connections: WebSocketConnections--WebSocket连接注册表
    next_request_id: Arc<AtomicU64>--下一个HTTP请求ID
    next_websocket_id: Arc<AtomicU64>--下一个WebSocket连接ID
    body_limit: usize--请求体上限
    response_start_timeout: Duration--响应开始超时
    stream_buffer_capacity: usize--HTTP流缓冲容量
    websocket_buffer_capacity: usize--WebSocket通道容量

WebSocketRouteState：WebSocket路由状态，私有结构体
    bridge: ServerBridgeState--服务器桥接状态
    classifier: ErasedWebSocketClassifier--当前路由的消息分类器

CompletedLoop：WebSocket根循环完成方，run_websocket_connection内私有局部枚举
    Read(Option<WebSocketCloseReason>)--读取循环先结束
    Write(Option<WebSocketCloseReason>)--写入循环先结束
```

## 函数

私有：
```text
start_server(world: &mut World)
    启动服务器System：从World取得配置与资源，冻结路由，构造停止通道并将run_server作为长期异步服务提交

build_router(router: Router, event_routes: Vec<EventRoute>, websocket_routes: Vec<WebSocketRoute>, bridge: ServerBridgeState) -> Router
    构建Router：将HTTP事件Handler和WebSocket升级Handler按路由合并到原生Router

run_server(options: ServerOptions, router: Router, handle: ServerHandle, event_sender: RuntimeEventSender, shutdown_receiver: oneshot::Receiver<()>)
    运行服务器：私有异步函数，绑定TCP Listener，发送ServerStarted、ServerFailed和ServerStopped，并执行有超时的优雅停止

handle_event_request(State(state): State<ServerBridgeState>, request: Request) -> Response<Body>
    处理HTTP委托请求：私有异步函数，缓冲受限Body，发送HttpRequestReceived，等待完整响应或流式响应开始

is_body_limit_error(error: &axum::Error) -> bool
    检查Body上限错误：私有函数，遍历error source链查找LengthLimitError

handle_websocket_upgrade(State(state): State<WebSocketRouteState>, upgrade: WebSocketUpgrade) -> impl IntoResponse
    处理WebSocket升级：私有异步函数，分配连接ID并将升级后的socket交给run_websocket_connection

run_websocket_connection(route: WebSocketRouteState, connection_id: WebSocketConnectionId, socket: WebSocket)
    运行WebSocket连接：私有异步函数，注册发送器，并发运行读写循环，结束时清理并发送断开事件

write_websocket_messages(writer: &mut SplitSink<WebSocket, Message>, messages: &mut mpsc::Receiver<Message>) -> Option<WebSocketCloseReason>
    WebSocket根写入循环：私有异步函数，从有界通道串行写入消息，Close后返回关闭原因

read_websocket_messages(event_sender: &RuntimeEventSender, classifier: ErasedWebSocketClassifier, connection_id: WebSocketConnectionId, sender: WebSocketSender, stream_buffer_capacity: usize, reader: &mut SplitStream<WebSocket>) -> Option<WebSocketCloseReason>
    WebSocket根接收循环：私有异步函数，处理Ping、Pong和Close，将文本或二进制消息交给route_websocket_message

route_websocket_message(event_sender: &RuntimeEventSender, classifier: &dyn WebSocketMessageClassifier, connection_id: WebSocketConnectionId, stream_buffer_capacity: usize, streams: &mut HashMap<WebSocketStreamId, WebSocketStream>, message: Message)
    分流WebSocket消息：私有异步函数，普通消息发事件，Start创建支流并发一次打开事件，Chunk、End和Abort只进入支流通道

remove_type_index(state: &mut WebSocketConnectionsState, connection_type: &str, connection_id: WebSocketConnectionId)
    移除连接类型索引：私有函数，从指定类型集合删除连接ID，并在集合为空时删除类型键
```

## 逻辑

```text
构建与启动：
    app.add_plugin(RuntimePlugin)
        -> app.add_plugin(AsyncRuntimePlugin)
        -> app.add_plugin(ServerPlugin)
        -> 其他Plugin注册原生路由或事件委托路由
        -> app.run
    第一次tick执行STARTUP
        -> RouteRegistry::freeze取出所有路由
        -> build_router生成最终Router
        -> spawn_async_service提交run_server

原生Axum路由：
    HTTP请求 -> Axum Handler自行处理 -> Axum直接返回响应

HTTP事件委托：
    HTTP请求
        -> Handler受限缓冲Body
        -> 创建HttpResponseSession并发送HttpRequestReceived
        -> Runtime被唤醒
        -> 同步System读取请求
        -> respond一次完整响应，或start_stream开始流式响应
        -> 开始响应前超时返回504

HTTP流式响应：
    同步System调用start_stream
        -> 将HttpResponseSession转交给异步任务
        -> 异步任务多次await send_chunk
        -> 通道已满时按客户端消费速度背压
        -> 会话可在LLM任务、工具事件和后续任务间多次转交
        -> 最终必须显式finish或abort
    全部会话句柄在未结束时丢失 -> Body输出abandoned错误
    客户端断开 -> 后续send_chunk返回RequestClosed

WebSocket普通消息：
    连接建立
        -> ServerPlugin先将名称和连接类型均为空的WebSocketSender写入WebSocketConnections
        -> 发送WebSocketConnected通知
        -> System可按ID设置连接类型、生成唯一名称或直接取得发送器
    普通消息
        -> 根接收循环发送WebSocketMessageReceived
        -> 同步System处理
        -> try_send一次立即回复，或将发送器克隆交给异步任务多次send
    连接断开
        -> 先从WebSocketConnections的ID、名称和类型索引删除
        -> 再发送WebSocketDisconnected通知

WebSocket连接筛选：
    get_all返回全部当前连接
    get_by_type按connection_type返回一组连接
    get_by_name按唯一名称返回单个连接
    get_unnamed只表示名称为空，不表示全部连接，不作为广播接口

WebSocket默认支流信封：
    {"mecs_stream":{"id":"request-1","phase":"start|chunk|end|abort"},"payload":...}
    不含mecs_stream -> Ordinary
    Start -> 创建有界支流通道，只发送一次WebSocketStreamOpened
    Chunk -> 直接写入支流通道，不进入ECS事件队列
    End -> 写入最后消息并标记Finished
    Abort -> 写入最后消息并标记Aborted

连接注册：
    客户端发送connection.register协议请求
        -> DtoPlugin转换为RegisterConnection事件
        -> ConnectionPlugin消费事件并写入WebSocketConnections的类型和名称索引
    ServerPlugin只负责保存和查询连接，不解析客户端业务协议

WebSocket流式接收：
    同步System读取WebSocketStreamOpened
        -> take取得唯一WebSocketStreamReceiver
        -> 将接收器移入异步累积任务
        -> 异步任务持续recv和累积
        -> End后返回累积结果给同步事件
    Abort或连接断开 -> recv只返回一次终止错误

并发与背压：
    多个WebSocketSender可并发填充同一条连接通道
    根写入循环串行发送，单条消息边界保持完整
    并发生产者的全局顺序不作保证，业务需要时自行携带sequence
    一条接收支流通道已满时，根接收循环等待容量，形成连接级背压

ServerPlugin边界：
    HTTP请求Body在进入ECS事件前完整缓冲
    HTTP响应支持一次完整返回和多次流式发送
    WebSocket普通消息通过ECS事件同步处理，支流通过异步通道累积
    流分片不进入ECS事件队列，事件只传递连接、普通消息和支流打开等状态
    流式HTTP上传、原生Axum提取器和中间件使用原生路由
    文件存储与落盘、prompt、token、tool call和业务协议不属于ServerPlugin
```

## 持有关系

```text
App
└── World
    ├── RuntimeHandle
    ├── AsyncRuntimeHandle
    ├── ServerOptions
    ├── RouteRegistry
    │   └── Mutex<RouteRegistryState>
    │       ├── Router
    │       ├── Vec<EventRoute>
    │       └── Vec<WebSocketRoute>
    ├── WebSocketConnections
    │   └── Arc<RwLock<WebSocketConnectionsState>>
    └── ServerHandle
        └── Arc<ServerHandleState>

AsyncRuntime专用线程
└── Axum长期服务Future
    ├── Router
    ├── ServerBridgeState
    └── 优雅停止接收端

每个HTTP委托请求
├── Axum Handler
│   └── 响应开始接收端
└── HttpRequestReceived
    └── HttpResponseSession
        └── Arc<Mutex<HttpResponseState>>

每条WebSocket连接
├── Axum连接任务
│   ├── 根接收器
│   ├── 根写入器
│   └── HashMap<WebSocketStreamId, WebSocketStream>
├── WebSocketConnections条目
│   └── WebSocketSender
└── 每条活动支流
    └── WebSocketStreamReceiver--唯一消费者，可转交不可克隆
```
