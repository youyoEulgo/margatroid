# Margatroid CLI

catalog、注册启动和成员管理接线已恢复并完成对应验证；创建、重命名、删除及发现能力已接通。验证结论限于实际执行场景，不将 API 存在或历史测试等同于普遍可用性保证。

## 用法

```text
margatroid workspace up [NAME] [--backend WS_URL]
```

`NAME` 是已注册工作区名称，不是 YAML 路径。显式名称可在任意目录使用，例如 `margatroid workspace up demo`。`--backend` 默认 `ws://127.0.0.1:3939/ws`。

CLI 连接并注册为 `cli`，随后查询目标 daemon 的权威 catalog。省略名称时，只匹配**规范化现有 CWD 与规范化 project_root 完全相等**的条目，不遍历祖先、不按前缀或子目录匹配：

- 一个匹配：启动该项。
- 多个匹配、交互终端：显示编号、名称、项目根和生命周期；选择一个编号后启动。空输入、`q` 或关闭信号取消，不发送启动请求；非法编号报错，不启动。
- 多个匹配、非交互终端：非零退出，提示提供名称；不会读取管道猜测选择。
- 无匹配或未知名称：非零退出，提示显式提供已注册名称。

目录匹配要求 CLI 与 daemon 共享文件系统命名空间；远程后端应使用显式名称。CLI 只规范化项目根，不读取任意 YAML、不扫描注册目录、不创建项目产物。

## 协议与权威定义

注册后发送 `workspace.catalog.query {id,message:{}}`，消费匹配 ID 的 `workspace.catalog {id,entries}`。选择后仅发送：

```json
{"type":"workspace.start","id":"request-id","message":{"workspace":"workspace:local/demo:latest","expected_revision":"catalog-revision"}}
```

定义读取、修订冲突校验、编译及启动属于 daemon。无效条目报告错误并提示修复注册定义和刷新。规范定义位于 `~/.margatroid/workspaces/<name>/workspace.yaml`；`project_root` 是显式绝对路径，成员状态属于工作区 home，项目内不创建 `.margatroid`。定义没有特殊管理成员或自定义数据库路径，空成员工作区合法。

## 会话与关闭

启动前注册和 catalog 等待有 5 秒超时，处理 Ping/Pong、二进制回执及关闭信号；交互选择期间也继续处理心跳和日志。stdin 仅用于启动前的编号选择，不提供 LLM 消息输入。

- 匹配的 `workspace.started` 表示工作区已建立：显示成功及每项 `member_failures` 的 Agent、Driver、错误，即使全部成员失败仍保持连接。
- 成功前匹配的 `workspace.start_failed` 或 `request.failed` 报错退出，不发送停止请求；成功后迟到或重复启动失败不结束会话。
- 首次 Ctrl+C/SIGTERM 发送 `workspace.stop`；启动尚未完成则保留停止意图，成功后再停止。
- 等待匹配 `workspace.stopped` 后关闭连接；停止失败、5 秒超时、重复关闭信号或等待期间断线报错退出。

后端 `log` 输出到 stdout，CLI 生命周期事件输出到 stderr，统一使用 RFC 3339 UTC 时间、等级和目标。终端启用 ANSI 等级色；重定向为无 ANSI 的纯文本。

## 验证

恢复后须用 `cargo test -p margatroid_cli` 验证参数、精确规范路径/符号链接、显式名称、零/多匹配非交互失败、编号/取消输入、心跳、匹配回执、启动拒绝，以及成员失败后会话和启动中 SIGTERM 的停止流程。

历史结果不代表当前树通过；本次未运行测试或构建。
