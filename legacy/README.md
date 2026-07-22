# 旧实现边界

`legacy/` 统一保存 V1/V2 参考实现，已经从 Cargo workspace 和正式运行入口中排除：

```text
legacy/
├── runtime_v2/  # 最后一版旧协作 runtime
├── server/      # 旧 Axum server 与业务路由
├── bridge/      # 旧远程 bridge
├── mcp_client/  # 旧 MCP client
└── docs/        # V1/V2 架构、迁移计划和 TaskChain 说明
```

更早且已被 `runtime_v2` 完全替代的 `runtime/` 源码不再保留；需要时从 Git 历史查看。

它们用于查询旧行为、数据结构和迁移背景，不再作为产品代码维护：

- 不在这些目录中增加新功能。
- 不为 V3 需求修补旧实现。
- 不允许 V3 crate 新增对这些目录的依赖。
- 需要复用的能力应重新确定职责边界后迁入对应 Plugin。
- 行为已经由 V3 替代后，可以删除对应旧目录；历史细节仍可从 Git 获取。

当前正式入口：

```text
daemon/src/main.rs
→ core_plugin::App
→ margatroid_defaults::MargatroidDaemonPlugins

cli/src/main.rs
→ HTTP
→ margatroidd
```

仍留在 workspace 的 `types`、`providers`、`compose`、`assets`、`paths` 和 `sandbox`
不是入口层旧实现；现有 V3 Plugin 仍依赖其中的能力，后续按 Plugin 边界逐步整理。
