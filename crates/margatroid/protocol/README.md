# margatroid_protocol

`margatroid_protocol` 定义 Margatroid CLI 与 daemon 共同使用的稳定纯数据协议。

该 crate 只依赖 `serde`，不依赖 ECS、Axum、CLI 或 daemon 实现。公开内容包括：

- 类型化的 Workspace、Request、Task、Agent 和 Resource ID
- API 与配置 schema version
- `WorkspaceSpec`、`WorkspaceBundle` 和资源清单
- workspace、prompt、request、task 和 result DTO
- 稳定错误码与 JSON error envelope

协议对象禁止携带 API key、token 或 Provider secret。资源内容使用内容摘要关联，daemon
必须在接受 bundle 前重新校验 schema、摘要、大小和引用关系。

当前 `crates/margatroid/types` 是旧内部类型库，不属于此公开协议，也不会整体迁入。
