# Margatroid Axum

`axum_plugin`是Margatroid的服务装配基础设施。它合并原生`axum::Router`，通过普通`send_async_closure`运行整个服务，不使用mecs服务器、通用HTTP/WebSocket事件或长期异步服务接口。

先安装Runtime、AsyncRuntime、ClosurePlugin及UPDATE闭包System，再安装`AxumPlugin::bind(SocketAddr)`。各插件在build中调用`AppAxumExt::add_axum_routes`，提交已经完成State绑定的Router；STARTUP取走Router后禁止继续变更。handler自行选择直接响应、专用通道或领域事件，不由基础设施规定。

`AxumHandle`提供`local_address`、`lifetime`、`request_shutdown`、`wait_stopped`、`is_stopped`和`result`。监听使用ConnectInfo注入真实对端SocketAddr。停止会取消共享信号并等待普通HTTP请求完成，没有固定超时后丢弃工作的逻辑。

WebSocket等升级工作不属于普通HTTP请求的退出范围。handler必须在on_upgrade之前取得`AxumLifetime::track()`的RAII guard，将它移动到回调，并持有到socket读写及其子任务真正结束。升级失败也应释放guard。服务只有在HTTP graceful shutdown和全部guard释放后才完成；guard不能在spawn子任务后提前释放。

App释放时独立资源只请求停止，不同步阻塞join。需要证明领域和服务安全退出时，调用`compose::shutdown_app`并保持World推进；仅drop App或强制终止进程不构成清理证明。排队闭包不预先捕获事件发送器，调度时使用AsyncContext取得通知能力，防止队列自持。

此crate不管理Client身份、连接名称、WS帧分类、协议Envelope或业务请求。DTO拥有/ws连接和协议，UI直接返回静态资源；业务领域通过client_core的发送能力出站。mecs不提供服务器封装和长期任务专用接口。
