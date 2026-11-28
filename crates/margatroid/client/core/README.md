# client_core

`client_core` 提供可共享的连接槽位、身份、出站队列与当前状态来源。
它依赖 mecs core 和 Tokio 通用能力，适合网络适配与领域更新共同持有。

## 核心对象

`Clients` 的 clone 共享连接注册表和状态来源；`same_instance` 可核对共享关系。
`ClientSender` 提交帧，`ClientReceiver` 由连接 writer 持有。
`SyncContext<T>` 组合快照读取与编码回调，使用 `SyncKey` 标识 topic/object，
以 revision 发布当前状态。`UpdateProbe` 和 `ProbeFactory` 为领域写入口建立观察能力。

## 分配连接并交付一帧

下面展示传输适配的最小生命周期。实际应用通过 client_plugin 处理注册身份。
示例函数可放入依赖 `client_core` 的包，并由 Tokio runtime 调用。

```rust
use client_core::{ClientFrame, Clients};

async fn deliver() {
    let clients = Clients::default();
    let shared = clients.clone();
    assert!(clients.same_instance(&shared));
    let (connection, mut receiver, cancel) = clients.allocate().unwrap();
    let sender = shared.get(connection).unwrap();
    sender.try_send(ClientFrame::Text("ready".into())).unwrap();
    assert_eq!(receiver.recv().await, Some(ClientFrame::Text("ready".into())));
    clients.remove(connection);
    assert!(cancel.is_cancelled());
    receiver.close().await;
}
```

`allocate` 创建容量为 128 的连接队列；`adopt` 可接入调用方创建的 sender。
`claim` 返回在途请求 ticket，最后一个 ticket 释放时删除对应请求标识。
通过选择器交付业务帧前，连接应完成身份登记。

## 发布当前状态

接入时为对象创建 SyncContext：快照回调返回 `(revision, owned_snapshot)`，
编码回调把快照转成共享 ClientFrame。实际写入后调用 `publish(revision)`，
来源结束时调用 `close()`。完整代码见 [共享状态测试](tests/shared.rs)。
测试也演示后加入连接获得最新状态、来源替换和 receiver 等待 worker 收口。
日志与流片段使用 `Clients::send_ordered` 保留具体帧的次序。

## 查阅与验证

- [公开入口](src/lib.rs)：队列、同步上下文和 World 查询扩展。
- [共享槽位](src/shared.rs)：allocate、身份登记、选择与来源发布。
- [出站队列](src/output.rs)：状态合并、worker 和容量错误。
- [共享状态测试](tests/shared.rs)：当前状态和有序帧调用。
- [Plugin 接入](../plugin/README.md) 与 [设计](../DESIGN.md)。

执行 `cargo test -p client_core` 验证队列与来源生命周期。
