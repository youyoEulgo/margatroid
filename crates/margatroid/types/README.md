# Margatroid Types

`margatroid_types` 保存多个 Margatroid Plugin 共享、没有运行行为的领域值类型。它不依赖 ECS，
也不承载 CLI/daemon 网络 DTO。

目前提供 `ResourceName` 和 `AgentImageReference`：

```rust
use margatroid_types::ResourceName;

let name = ResourceName::new("local/code-review")?;
assert_eq!(name.scope(), "local");
assert_eq!(name.name(), "code-review");

let image = margatroid_types::AgentImageReference::new("local/coder")?;
assert_eq!(image.to_string(), "local/coder:latest");
```

资源名称固定使用 `scope/name`。Skill、Workflow 和 AgentImage Loader 共享这一类型，从而不会各自
维护一套名称解析规则。AgentImage 引用在名称后增加可选 `:tag`，省略时规范化为 `latest`。
