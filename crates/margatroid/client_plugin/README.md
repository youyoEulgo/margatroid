# ClientPlugin

`ClientPlugin` 消费 ServerPlugin 的 `RegisterConnection`，校验客户端声明的类型，在
ServerPlugin 的 `WebSocketConnections` 中写入连接类型和唯一名称，并把注册成功的客户端实体化为
`Client` Entity；连接断开时清除该 Entity。

客户端类型只允许小写 ASCII 字母、数字、下划线和短横线。资源 ID 由后端构成：

```text
client:<client_type>/<name>:<connection_id>
```

`name` 是客户端注册时可选提供的名称，省略时取连接句柄的十进制文本；`connection_id` 由后端分配。
例如连接 ID 为 `12` 的 Web UI 提供名称 `console` 后，会得到资源 ID `client:webui/console:12`。

`Client` Entity 必须挂载 `Client` 组件和 `ResourceId` 组件。类型与名称都只是客户端自报的标签，
只用于构造可读资源 ID 与出站路由，不表示认证结果或权限。

每次注册都会发出 `ClientRegistrationResult`，携带请求 ID、连接 ID 与 `Ok(RegisteredClient)` 或
`Err(ClientError)`。回执在实体创建之后才发出，因此等待回执再发后续请求的客户端一定能看到自己的
实体；注册失败也由此第一次能传达给客户端，而不只是写服务端日志。

`registered_client(world, connection_id)` 是同一事实的公开查询，返回已注册客户端的 `ResourceId`，
未注册时返回 `None`。DtoPlugin 用它做注册检查，拒绝未注册连接发出的其它请求。

`client_source(client_type, name, connection_id)` 是同一套标识的公开构造函数，供统一 MCL 来源审计使用：
类型与名称都存在时返回 `client:<client_type>/<name>:<connection_id>`，与客户端实体的资源 ID 相同；
连接未注册时返回 `unregistered:<connection_id>`，因为未注册的连接没有实体。
