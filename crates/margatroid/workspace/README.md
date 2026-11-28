# Workspace

Workspace 把项目目录、Agent 持久登记和运行实体组织在同一管理域中。
首次接入可从 [workspace_plugin](plugin/README.md) 的 App 安装和创建事件开始；
直接操作登记库时，阅读 [workspace_core](core/README.md)。
结构、持有关系和函数行为见 [DESIGN](DESIGN.md)。

## 先认识三个身份

| 概念 | 含义 | 定义 |
| --- | --- | --- |
| address / node_name | 运行树上的节点身份，用于定位 Workspace 与 Agent | [事件](core/src/events.rs) |
| id | Agent 登记的持久身份，归档后仍可用于查找记录 | [WorkspaceAgent](core/src/types.rs) |
| alias | 数据库保存的展示名称，可按持久 id 修改 Agent 别名 | [数据库](core/src/database.rs) |

`Workspace` 组件持有 `project_root`、运行状态和 `WorkspaceDatabase`。
实体地址由 Address 组件提供，Workspace 与 Agent 的别名由数据库确认快照提供。
Agent 的 `Normal`、`Temporary`、`Archived` 类别描述登记及目录归属，
运行状态由对应运行实体表达。

## 管理目录与项目目录

管理根由 `WorkspacePlugin.root` 指定，单个 Workspace 使用下列布局：

```text
<管理根>/<节点名>/
  workspace.sqlite
  agents/
    normal/<node_name>/
    temporary/<独立id>/
    archived/<归档目录名>/
```

`project_root` 指向实际项目目录，管理目录与项目目录要求各自独立。
创建事件使用单层根地址，例如 `/demo`；数据库保存绝对项目路径和 Driver 工具入口。
Agent 目录内的存储由 [Storage](../storage/README.md) 负责。

## 一次创建与启动

1. `CreateWorkspace` 准备管理目录和登记库，随后产生 `StartWorkspace`。
2. 装配流程挂载 Address、Workspace 和 ToolTasks，再回复 Workspace Entity。
3. `StartWorkspaceAgent::New` 将 Driver 工具路径和 JSON 参数交给工具执行流程。
4. Driver 创建注入流程持有数据库操作凭证，准备 Agent 文件并提交登记，再完成实体启动。
5. `Existing` 使用已登记 Driver 入口和调用参数，按当前 node_name、alias 整理启动参数。

创建、启动、删除与归档的完整运行案例见 [生命周期测试](plugin/tests/lifecycle.rs)。
数据库 schema 要求 v1，打开时完成校验与已登记归档意图恢复后提供句柄。

## 提交、归档与关闭

`WorkspaceRecordsSnapshot` 同时携带 metadata 和全部 Agent 登记，revision 随实际提交变化递增。
`subscribe_changes()` 提供 watch，`metadata()` 读取确认快照中的 Workspace 元数据。
展示别名修改使用 `rename_workspace_alias` 与 `rename_agent_alias`；
Agent 别名为 `None` 时表示清空，节点身份和目录继续沿用登记值。

归档先等待对应任务与 Agent 收口，再持久登记归档意图、移动数据目录和提交 Archived。
目录移动后的提交故障返回 `ArchivePending`，后续恢复沿同一意图继续。
删除先进入 Deleting 并关闭受理，等待任务与数据库操作完成，
再关闭数据库、移除运行实体并删除管理目录；项目目录保持原有归属。
错误进入可诊断状态，调用方通过原始 oneshot 取得结果。
