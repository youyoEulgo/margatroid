# Margatroid V3 产品协议 API

状态：阶段 1 第一版已实现

实现 crate：`crates/margatroid/protocol`（package：`margatroid_protocol`）

## 1. 边界

协议 crate 是 CLI 与 daemon 共同依赖的纯数据层：

- 只依赖 `serde` 和 `serde_json`。
- 不依赖 ECS、Axum、CLI、daemon 或任何 Plugin。
- 不包含文件读取、网络请求、状态持久化和业务调度。
- 不复用旧 `types` crate 中的 bridge、MCP、Provider 或 V2 runtime 类型。
- 协议对象禁止携带 API key、token、Authorization header 或 Provider secret。

## 2. 版本

```text
API_VERSION = "v1"
CURRENT_SCHEMA_VERSION = 1
```

- `API_VERSION` 用于 HTTP 路径，例如 `/v1/workspaces`。
- `SchemaVersion` 是 JSON number，当前 `WorkspaceBundle.schema_version` 必须为 `1`。
- CLI 可以解析和构建当前 schema；daemon 必须再次检查 `is_supported()`。
- 新增 optional 字段、enum variant 或 response 字段时保持同一 API major version。
- 删除字段、改变字段含义或 JSON 类型时必须升级 API/schema major contract。

## 3. 稳定 ID

```text
WorkspaceId
RequestId
TaskId
AgentId
ResourceId
ProjectName
```

这些类型在 JSON 中均为 string，不绑定 UUID、ULID 或数据库实现。构造和反序列化统一校验：

- 非空，最长 128 bytes。
- 不能是 `.` 或 `..`。
- 不能包含 `/`、`\\` 或空白字符。
- 业务关联只使用类型化 ID，不依赖队列顺序或数组位置。

## 4. Workspace 与资源包

```text
WorkspaceBundle
├── schema_version: SchemaVersion
├── spec: WorkspaceSpec
├── manifest: ResourceManifest
└── resources: BundledResource[]

WorkspaceSpec
├── project: ProjectName
├── description?: string
├── manager: AgentId
├── agents: WorkspaceAgentSpec[]
└── workflows: ResourceReference[]
```

`ResourceReference` 使用显式 tagged union：

```json
{ "source": "installed", "id": "resource-1" }
```

或者：

```json
{
  "source": "bundled",
  "digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
}
```

`ContentDigest` 当前只接受规范化的 `sha256:` 加 64 位小写十六进制。`BundledResource`
使用 `content_base64`，避免二进制在 JSON 中展开为整数数组。

协议类型只固定传输形状，不负责以下权威校验：

- manifest entry 与 resources 是否一一对应。
- digest、`size_bytes` 和解码后内容是否匹配。
- manager、Agent、Workflow 和已安装 Resource 引用是否存在。
- bundle 大小、media type、资源删除和安全策略。

这些校验由阶段 3 的项目编译器预检，并由阶段 4 的 daemon 再次执行。

## 5. Workspace DTO

```text
CreateWorkspaceRequest  { bundle }
CreateWorkspaceResponse { workspace }
ListWorkspacesResponse  { workspaces[] }
WorkspaceSummary
```

`WorkspaceStatus` 当前包含：

```text
created → starting → running → stopping → stopped
                    ↘ failed ↙
```

具体状态转换由阶段 4 的 WorkspacePlugin 定义；协议当前只固定状态名称。
所有 `*_at_ms` 字段均为 Unix epoch milliseconds。

## 6. Request 与 Task DTO

```text
SubmitPromptRequest  { prompt }
SubmitPromptResponse { request }
GetRequestResponse   { request, tasks[] }
RequestSummary
TaskSummary
TaskResult
```

Request 和 Task 共用 `ExecutionStatus`：

```text
queued ──→ running ──→ completed
  │           │  ↑
  │           ↓  │
  │         waiting
  │           │
  └───────────┴────→ failed | cancelled
```

允许重复写入同一状态以支持幂等更新。`completed`、`failed`、`cancelled` 是 terminal，
不能回到运行态。协议通过 `is_terminal()` 和 `can_transition_to()` 固化该规则。

`TaskResult` 只包含产品可见的文本结果、Resource ID 形式的 artifact 引用和完成时间；
Provider 原始响应、私有 prompt 和 secret 不属于产品结果协议。

## 7. 错误协议

所有 HTTP 错误使用：

```json
{
  "error": {
    "code": "resource_in_use",
    "message": "resource is still referenced",
    "request_id": "request-1"
  }
}
```

稳定错误码与 HTTP status：

| ErrorCode | HTTP |
|---|---:|
| `invalid_argument` / `invalid_bundle` / `unsupported_version` | 400 |
| `unauthorized` | 401 |
| `forbidden` | 403 |
| `not_found` | 404 |
| `already_exists` / `resource_in_use` / `conflict` | 409 |
| `queue_full` | 429 |
| `unavailable` | 503 |
| `internal` | 500 |

`message` 面向人类且不作为程序判断依据；客户端只根据 `code` 和 HTTP status 分支。
`details` 是可选结构化上下文，不得包含 secret。

## 8. 测试契约

协议 crate 必须持续通过：

- ID 构造与反序列化校验测试。
- `WorkspaceBundle` 精确 JSON shape 与双向 serde 测试。
- Request 状态和 ErrorCode snake_case 测试。
- ExecutionStatus 状态迁移测试。
- 无 ECS、Axum、CLI、daemon 或 legacy 依赖的 metadata 检查。

本地执行 `scripts/check-protocol-boundary.sh` 检查依赖边界和 CLI/daemon 接入。
