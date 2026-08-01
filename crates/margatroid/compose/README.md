# margatroid_compose

`margatroid_compose` 是 `margatroid-workspace.yaml` 项目的本地编译器。

它解析并校验用户编写的 YAML，解析项目级目录和主目录中的 Skill、Workflow 包，最终生成
确定性的 `margatroid_protocol::WorkspaceBundle`。

该 crate 不启动 ECS、不连接 daemon、不执行 Workflow，也不读取 Provider 密钥。
