# ConnectionPlugin

`ConnectionPlugin` 消费 ServerPlugin 的 `RegisterConnection`，校验客户端声明的类型，并在
ServerPlugin 的 `WebSocketConnections` 中写入连接类型和唯一名称。

客户端类型只允许小写 ASCII 字母、数字、下划线和短横线。连接名称由后端生成：

```text
{client_type}-{connection_id}
```

例如连接 ID 为 `12` 的 Web UI 会注册为类型 `webui`、名称 `webui-12`。该类型只是消息路由标签，
不表示认证结果或权限。
