# Client 迁移到 providers crate

## 完成时间
2026-06-11

## 动机

runtime_v2 中的 member.rs 和 workspace.rs 通过 `#[path]` 借用旧 runtime 的 client.rs，导致：
- runtime_v2 依赖旧 runtime 的文件路径
- 代码耦合，不利于独立演进

## 解决方案

将 Client 从 `runtime/src/client.rs` 迁移到 `providers/src/client.rs`

### 方案对比

| 方案 | 优点 | 缺点 | 决策 |
|------|------|------|------|
| 放进 providers | 语义合理、减少 crate 数量、依赖清晰 | providers 职责扩大 | ✅ 选择 |
| 独立 llm_client crate | 职责单一、灵活性高 | crate 碎片化、可能过度设计 | ❌ |
| 保持借用 | 快速验证架构 | 代码耦合、路径依赖 | ❌ |

**最终选择**：放进 providers crate

**理由**：
1. Client 本质上是 provider 层的一部分（封装 DynAiProvider）
2. 依赖关系天然：runtime → providers::Client → providers::OpenRouterProvider
3. Client 很轻量（约 250 行），与 provider 实现放一起合理
4. 符合分层原则：types (接口) → providers (实现+封装) → runtime (使用)

## 实施步骤

1. ✅ 复制 `runtime/src/client.rs` → `providers/src/client.rs`
2. ✅ 更新 `providers/src/lib.rs`：添加 `pub mod client; pub use client::Client;`
3. ✅ 添加 `tracing = "0.1"` 到 `providers/Cargo.toml`
4. ✅ 更新 `runtime_v2/src/member.rs`：`use providers::Client;`
5. ✅ 更新 `runtime_v2/src/workspace.rs`：`use providers::Client;`
6. ✅ 添加 `providers = { path = "../providers" }` 到 `runtime_v2/Cargo.toml`

## 验证

```bash
# providers 编译成功
cargo build -p providers
# ✅ Finished `dev` profile

# runtime_v2 测试全部通过
cargo test -p runtime_v2
# ✅ test result: ok. 20 passed; 0 failed

# 旧 runtime 仍然正常编译（使用本地 client.rs）
cargo build -p runtime
# ✅ Finished `dev` profile
```

## 影响范围

### 新增
- `providers/src/client.rs` (251 行)
- `providers` 依赖 `tracing`

### 修改
- `providers/src/lib.rs`：导出 Client
- `runtime_v2/src/member.rs`：删除 `#[path]` 借用，改用 `use providers::Client`
- `runtime_v2/src/workspace.rs`：删除 `#[path]` 借用，改用 `use providers::Client`
- `runtime_v2/Cargo.toml`：添加 `providers` 依赖

### 不受影响
- 旧 `runtime` 仍使用本地 `client.rs`
- 所有测试保持通过

## API 变化

无破坏性变化。对外 API 保持不变：
```rust
// 旧方式（runtime_v2 内部）
use client::Client;  // 通过 #[path] 借用

// 新方式
use providers::Client;  // 从 providers crate 导入
```

外部使用者（server、cli）：
```rust
use providers::Client;  // 统一从 providers 导入
```

## 后续清理

当 runtime_v2 完全替代旧 runtime 后，可以删除 `runtime/src/client.rs`，届时所有组件统一使用 `providers::Client`。

## 收益

1. **解耦**：runtime_v2 不再依赖旧 runtime 的文件路径
2. **复用**：providers::Client 可被 runtime、runtime_v2、server、cli 共享
3. **语义清晰**：Client 作为 provider 层的一部分，命名空间更合理（`providers::Client`）
4. **维护性**：Client 的修改只需在一处进行

---

**作者**: Claude Opus 4.8  
**状态**: ✅ 完成
