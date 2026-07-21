# Margatroid V3 Plugin API 文档索引

状态：草案

原有的单一 Plugin API 文档已按稳定性和发布边界拆分：

- [V3-INFRASTRUCTURE-API.md](V3-INFRASTRUCTURE-API.md)：通用 ECS 与基础设施公开 API。
- [V3-BUSINESS-PLUGIN-API.md](V3-BUSINESS-PLUGIN-API.md)：Margatroid 业务 Plugin 契约。

通用 ECS 与基础设施体系暂定名为 **mecs**，以开发者友好、配置简单和
默认开箱即用为公开 API 的主要设计目标。

## 文档边界

基础设施文档覆盖：

```text
core_plugin
app_runtime_plugin
async_runtime_plugin
log_plugin（第一版已实现）
http_server_plugin（第一版已实现）
external_event_plugin（仅 API 设计，尚未实现）
```

这部分按未来可独立发布到 crates.io 的公开 API 标准维护，不允许引用 Margatroid 的
LLM、agent、workflow、workspace 等领域类型。

业务文档覆盖：

```text
config_plugin
event_bus_plugin
llm_plugin
sandbox_plugin
skill_plugin
server_plugin
未来的 workspace / workflow / member / memory plugin
```

这些 Plugin 可以依赖 Margatroid 领域模型，并通过事件和 Resource 组成完整产品。

## 语言策略

现阶段所有设计文档统一使用中文。正式准备公开发布时再设计英文文档与 i18n 策略，
现在不维护重复翻译，避免两份规范发生漂移。

## 规范优先级

出现冲突时：

1. 基础设施 public API 以 `V3-INFRASTRUCTURE-API.md` 为准。
2. 业务事件和 Resource 以 `V3-BUSINESS-PLUGIN-API.md` 为准。
3. 产品整体方向以 `V3-DESIGN.md` 为准。

任何 Stable API 的破坏性修改必须同步更新对应文档，并在 commit message 中标明
`Breaking changes`。
