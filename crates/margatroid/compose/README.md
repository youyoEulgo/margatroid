# Compose

`compose` 是 CLI 使用的 Workspace 文件编译器。它读取 `margatroid-workspace.yaml`，解析和校验
静态配置，返回不包含 ECS Entity、AgentImage 正文或资源正文的 `WorkspaceDefinition`。CLI 或
protocol 层可以继续把这个定义发送给后端的 `WorkspacePlugin`。

```rust
use compose::compile;

let definition = compile("margatroid-workspace.yaml")?;
// 将 definition 交给 CLI/protocol 的后端请求。
```

最小配置：

```yaml
name: demo
manager: coder
project_root: .
agents:
  coder:
    image: local/coder:latest
    resources:
      - provider: skill
        name: local/project-context
```

## 文件语法

Workspace 文件通常命名为 `margatroid-workspace.yaml`，顶层只能包含以下字段：

```yaml
name: demo                         # 必填，Workspace 名称
project_root: .                    # 可选，默认为 workspace 文件所在目录
manager: coder                     # 可选，默认为第一个 Agent
agents:                            # 必填，至少一个 Agent
  coder:
    image: local/coder:latest      # 必填，scope/name[:tag]
    resources:                     # 可选，额外启用的资源
      - provider: skill
        name: local/project-context
    disable_resources:             # 可选，禁用的资源
      - provider: skill
        name: local/dangerous-command
    memory_path: memory/coder.sql  # 可选，SQLite 路径
```

### 顶层字段

`name` 是 Workspace 的逻辑名称，不能为空，不能包含 `/`、`\\` 或控制字符，且不能是 `.` 或
`..`。

`project_root` 是项目根路径。相对路径以 workspace 文件所在目录为基准，绝对路径直接使用；省略
时使用 workspace 文件所在目录。路径会被转换成绝对路径，不能包含 `..`。

`manager` 指定默认入口 Agent，必须匹配 `agents` 中的 Agent 名称。省略时使用配置中第一个
Agent。

`agents` 不能为空。Agent 名称在同一个 Workspace 内必须唯一，并遵守和 `name` 相同的名称规则。

### Agent 定义

`agents` 推荐使用名称映射，映射键就是 Agent 名称：

```yaml
agents:
  coder:
    image: local/coder:latest
  reviewer:
    image: local/reviewer:v2
```

也可以使用列表，此时每项必须显式提供 `name`：

```yaml
agents:
  - name: coder
    image: local/coder
  - name: reviewer
    image: local/reviewer:v2
```

列表和映射都会按照文件中的出现顺序编译。映射形式下如果同时写了 Agent 内部的 `name` 字段，
它必须和映射键相同。

`image` 是 AgentImage 引用，格式为 `scope/name[:tag]`。省略 tag 时使用 `latest`：

```yaml
image: local/coder
# 等价于 local/coder:latest
```

### Resource 语法

`resources` 和 `disable_resources` 都是资源引用列表。资源引用由 `provider` 和 `name` 组成：

```yaml
resources:
  - provider: skill
    name: local/project-context
  - provider: workflow
    name: local/review
  - provider: tool
    name: builtin/read-file
```

也支持 `provider:scope/name` 简写，建议加引号：

```yaml
resources:
  - "skill:local/project-context"
  - "workflow:local/review"
```

`provider` 是工具定义 Plugin 的稳定 ID，只允许小写 ASCII 字母、数字、`_` 和 `-`。当前常见
Provider 包括 `tool`、`skill` 和 `workflow`，但编译器允许后续注册其他 Provider。

`name` 必须严格是 `scope/name`，两个部分都不能为空，不能使用 `.`、`..`，也不能包含反斜杠或
控制字符。例如 `local/project-context` 合法，`project-context`、`local/a/b` 和
`../dangerous` 非法。

`resources` 是在 AgentImage 默认资源上额外启用的资源；`disable_resources` 优先级最高，会从
最终可见资源中移除同名引用：

```text
镜像默认资源 + resources - disable_resources
```

### Memory 路径

`memory_path` 是该 Agent 的 SQLite 文件覆盖路径。相对路径以 `project_root` 为基准，绝对路径
直接使用；最终会转换为绝对路径并拒绝 `..`。省略时由后端 WorkspacePlugin 使用默认路径：

```text
<project>/.margatroid/workspaces/<workspace>/memory/<agent>/memory.sql
```

### 编译边界

配置包含未知字段、缺少必填字段或违反上述规则时，`compile` 返回错误，不会产生部分定义。Compose
只解析名称、引用和路径，不加载 AgentImage、Skill、Workflow、模型路由或 Memory，也不连接
daemon；CLI 或 protocol 层负责把编译得到的 `WorkspaceDefinition` 发送给后端。
