# DaemonLifecyclePlugin

Margatroid 专用的 daemon 生命周期与就绪策略。

该 Plugin 公开 `Starting`、`Ready`、`Draining` 和 `Stopped` 状态，并在共享 HTTP Server
上注册 `/ready`。信号处理和基础设施所有权仍由各自的 mecs Plugin 负责。
