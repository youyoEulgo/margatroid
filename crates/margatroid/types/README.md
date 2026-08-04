# Margatroid Types

`margatroid_types` 保存多个 Margatroid Plugin 共享、没有运行行为的领域值类型。它不依赖 ECS，
也不承载 CLI/daemon 网络 DTO。

目前只提供 `ResourceName`：

```rust
use margatroid_types::ResourceName;

let name = ResourceName::new("local/code-review")?;
assert_eq!(name.scope(), "local");
assert_eq!(name.name(), "code-review");
```

资源名称固定使用 `scope/name`。Skill、Workflow 和 AgentImage Loader 共享这一类型，从而不会各自
维护一套名称解析规则。
