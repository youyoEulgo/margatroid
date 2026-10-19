# ClientPlugin

`ClientPlugin` 消费 ServerPlugin 的 `RegisterConnection`，校验客户端类型和唯一名称，更新连接元数据，再通过统一受检入口创建挂载 `Client` 的 Entity；连接断开时清除该 Entity。`Client` 是共享 `margatroid_types::client_components` 中的 `LibraryComponent`，自身保存 `resource_id: ResourceID`，不挂独立 `ResourceId` 组件。

catalog、注册启动和成员管理接线已恢复并完成对应验证；创建、重命名、删除及发现能力已接通。验证结论限于实际执行场景，不将 API 存在或历史测试等同于普遍可用性保证。

客户端类型只允许小写 ASCII 字母、数字、下划线和短横线。后端构成规范地址：

```text
client:<client_type>/<name>:<connection_id>
```

`name` 可选，缺省为后端分配连接句柄的十进制文本。例如连接 `12`、类型 `webui`、名称 `console` 对应 `client:webui/console:12`。类型与名称是自报标签，用于可读地址和路由，不表示认证结果或权限。

每次注册通过 `ClientRegistrationResult` 携带请求 ID、连接 ID 与 `Result<RegisteredClient, ClientError>`。成功仅在 Entity 已可查询后报告；失败不得留下半注册实体。`WorldClientExt::client_by_connection` 返回 `Option<(Entity, &Client)>`，保留完整 Entity generation；`registered_client(world, connection_id)` 是返回 `Option<ResourceID>` 的便捷查询。

网络入口必须从当前真实连接取得已注册 Client，并绑定其完整 Entity、ResourceID 及连接上下文；后续异步结果须核对这些上下文，不能发送给复用 index 的新 Entity 或新连接。未注册请求拒绝，客户端不能覆盖 source；直接调用也必须显式提供并校验注册身份，不能隐式选取第一个 Client、Agent 或 Workspace。Driver 宿主调用则绑定真实 Agent 实例，目标参数不是来源。

当前仍导出 `client_source(client_type, name, connection_id) -> String`，缺少元数据时会产生 `unregistered:<connection_id>`。这只是旧字符串辅助函数，不验证注册、Entity generation 或连接存活；不能作为请求 source 的权威构造或未注册回退。目标身份入口应使用已注册共享组件，当前 DTO 接线已使用真实注册身份，完整 generation/连接关联有对应回归验证。

`client_webui_senders` 从已注册 webui Client 与 Server 连接表收集去重且未关闭的 Sender。推理提交前取得发送器快照，后来连接不补入；快照不按 Workspace 隔离，连接关闭不取消已提交推理，推流失败不改变权威 oneshot 结果。Server 拥有连接和 Sender 生命周期，ClientPlugin 只管理注册身份。
