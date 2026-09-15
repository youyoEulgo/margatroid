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
