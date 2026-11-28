# Client

Client 领域把传输连接、已登记身份和出站消息交付连接起来。
World、网络适配与异步领域观察共享同一个 `Clients` 槽位，
业务生产者按当前注册身份选择接收者，再通过连接自己的出站队列发送。

## 核心概念

- `ConnectionId` 标识一次传输连接；`Client` 组件提供 World 中的身份与实体寻址。
- `Clients` 保存连接发送器、注册身份、在途请求与状态来源，clone 共享同一实例。
- `ClientIdentity` 保存登记后的实体、地址、类型和名称。
- `ClientSelector` 按类型、名称或广播选择当前接收者。
- `ClientSender` / `ClientReceiver` 为传输 writer 提供有界出站队列。
- `SyncContext` 为当前状态提供快照准备、编码、revision 发布与来源关闭能力。

## 一次连接的生命周期

网络入口先调用 `Clients::allocate`，取得连接标识、接收器和取消令牌。
登记请求经 `RegisterClient` 提交给 ClientPlugin；系统校验类型和名称、创建实体，
再把同一身份写入共享槽位。成功登记后初始化符合选择条件的当前状态来源。

业务更新通过同一 Clients 选择身份并入队；writer 从 receiver 取出消息后写入传输。
传输结束时移除槽位并提交 `ClientDisconnected`，插件清理对应 World 实体。
完整登记与断连的可运行调用见 [注册测试](plugin/tests/registration.rs)。

## 选择交付方式

| 数据 | 入口与行为 |
| --- | --- |
| 当前对象状态 | SyncContext 发布 revision，队列合并同来源更新，准备最新快照 |
| 持久记录、日志、流片段 | `send_ordered` 按具体帧顺序交付 |
| 带作用域的增量 | `send_scoped` 在作用域活跃时交付 |

队列达到容量或关闭时，发送返回 Full/Closed 并取消连接。
receiver 的异步关闭等待在途状态准备 worker 完成。
状态来源替换以 epoch 区分实例，revision 表达该来源的单调变化。

## 开发导航

- [client_core](core/README.md)：共享连接、队列和同步来源的使用入口。
- [client_plugin](plugin/README.md)：登记与断连系统的安装和调用。
- [DTO](../dto/README.md)：协议请求与领域对象的适配。
- [Axum](../axum_plugin/README.md)：WebSocket 连接和 writer 生命周期。
- [设计](DESIGN.md)：连接共享状态与消息交付结构。

运行 `cargo test -p client_core -p client_plugin` 验证交付和身份生命周期。
