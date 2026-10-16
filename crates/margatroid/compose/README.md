# Compose

`compose` 是 CLI 的 Workspace YAML 编译器。它返回共享 `workspace_components` 中的静态 `WorkspaceDefinition`，不创建 ECS Entity、不连接后端，也不加载驱动、资源正文或历史数据库。

```rust
use compose::compile;
let definition = compile("margatroid-workspace.yaml")?;
```

## 配置

```yaml
name: demo
project_root: .
manager: coder
agents:
  coder:
    driver: driver:local/coder:latest
    home_root: /home/example
    history_path: state/coder.sqlite
```

顶层只接受 `name`、`project_root`、`manager`、`agents`。未知字段被拒绝。

- `name` 必填，不能为空、不能为 `.` 或 `..`，不能包含斜杠、反斜杠或控制字符，最多 128 字节。
- `project_root` 可选，默认配置文件所在目录。相对路径以配置文件目录为基准。
- `manager` 在非空工作区中默认第一个成员名称，必须指向已配置成员。
- `agents` 必填，接受名称映射或列表，保留文件出现顺序。列表中每项必须提供 `name`；映射中可省略内部 `name`，若提供则必须与键一致。成员名称遵循同样规则且不能重复。

空工作区合法，编译后成员为空、manager 为空：

```yaml
name: empty
agents:	[]
```

空工作区不接受指向不存在成员的非空 manager；也可显式写 `manager: ''`。

## 成员字段

每个成员只接受 `name`、`driver`、`home_root` 和 `history_path`。

`driver` 必填，完整格式为 `driver:scope/name[:tag]`，省略标签时规范化为 `latest`。也接受省略 `driver:` 前缀的 `scope/name[:tag]` 输入，编译后始终保存纯值 `ResourceID`。

`home_root` 可选，默认真实环境变量 `HOME`。没有配置该字段且 HOME 缺失时编译失败。显式相对路径以 `project_root` 为基准。

`history_path` 可选，相对路径以 `project_root` 为基准。`memory_path` 仅作为此字段的输入别名保留，编译结果只有 `history_path`。默认路径由 Compose 生成：

```text
<project_root>/.margatroid/<workspace-name>/history/<agent-name>.sqlite
```

全部路径最终必须为绝对路径，移除当前目录组件并拒绝任何 `..`；编译不会创建目录或打开 SQLite。

## 地址与输出

- Workspace 地址：`workspace:local/<name>:latest`。
- 成员地址：`agent:<workspace-name>/<agent-name>:latest`。
- Agent 不支持独立 `tag` 字段；驱动标签只写在 `driver` 地址中。
- 输出保留 `name`、`manager`、`project_root`，成员保留 `id`、`name`、`driver`、`home_root`、`history_path`。

Workspace 文件不接受 `resources`、`disable_resources`、`tool_default` 或 `tool_dynamic` 等旧资源列表。资源导入及可见性属于运行期 Agent/MCL 契约，不由 Compose 生成默认资源或执行旧 `handle` 逻辑。

缺少必填字段、未知字段、重复成员、无效引用或路径错误都会返回 `ComposeError`，不产生部分定义。运行期 Workspace 插件仍须复核收到的定义。
