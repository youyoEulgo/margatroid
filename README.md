# Margatroid

用 Rust 实现的多智能体协作运行时。当前正在迁移到 V3 ECS + Plugin 架构。

项目仍在早期开发中，功能并不完善，API 和架构可能随时变动。

当前正式入口只使用 V3 ECS：

```bash
# 启动 daemon，默认监听 127.0.0.1:3939
cargo run -p margatroidd

# 通过 HTTP 检查 daemon
cargo run -p cli -- status
```

可通过 `MARGATROID_BIND` 修改 daemon 地址，通过 `MARGATROID_URL` 修改 CLI 连接地址。
当前入口只承诺已经落地的基础设施能力；workflow、workspace 和 prompt API 尚未接入。

## 项目结构

```text
apps/                  # 可执行程序入口
├── cli/               # margatroid 本地工具链与 daemon 客户端
└── daemon/            # margatroidd ECS 守护进程
crates/
├── mecs/              # 领域无关、计划独立发布的 ECS 与基础设施
└── margatroid/        # Margatroid 业务 Plugin 与领域 crate
docs/v3/               # V3 设计、API 契约与产品路线图
legacy/                # 已退出正式依赖图的 V1/V2 参考实现
```

- [V3 架构设计](docs/v3/V3-DESIGN.md)
- [mecs 公开 API](docs/v3/MECS-API.md)
- [Margatroid 公开 API](docs/v3/MARGATROID-API.md)
- [V3 产品路线图](docs/v3/V3-ROADMAP.md)
- [旧实现与历史文档](legacy/README.md)
