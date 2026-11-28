# Tool Resource

工具资源将磁盘上的工具定义登记到实体的 `Tools` 集合，供推理和工具执行方按 alias 查询。
开发入口按用途分为 [core](core/README.md) 与 [plugin](plugin/README.md)。

## 核心对象与资源结构

| 对象 | 用途 |
| --- | --- |
| `ToolInput` | 一次读取产生的描述、schema、Lua 文本和规范入口路径 |
| `ToolEntry` | 登记后的定义、入口路径及任务绑定信息 |
| `Tools` | 实体上的 alias 集合与注册生命周期凭证 |
| `ImportTool` | 携带 owner、路径、取消令牌和回复端的导入请求 |
| `ToolResourcePlugin` | 调度异步文件校验并发布登记结果 |

一个工具目录包含以下文件：

```text
sample/
  tool.toml          # schema_version = 1；description 为有效描述文本
  input.schema.json  # JSON Schema 对象，支持 # 开头的内部引用
  main.lua           # UTF-8 程序文本
```

`read_tool` 在 core 中完成文件读取和定义校验，包括 JSON Schema 编译检查。
导入后，集合保存描述和 schema；后续读取集合直接取得登记时的定义。
Lua 文本属于本次读取结果，执行流程由 [Tool 领域](../tool/README.md) 承担。

## 最小调用流程

1. 调用入口解析资源地址，准备存活实体、合法 alias 与绝对入口路径。
2. 安装运行时和 `ToolResourcePlugin`，发送 `ImportTool`。
3. plugin 的 blocking worker 调用 `read_tool`，等待读取完成后交回规范路径、描述和 schema。
4. ECS 结果入口复核 owner 与取消状态，通过 `Tools::insert` 原子登记，再回复 `Result<(), ToolResourceError>`。
5. 使用方通过 `WorldToolResourceExt::tool_entry` 或 `Tools::get` 取得定义。

相同 alias 的再次登记返回 `AliasConflict`。动态工具注册使用同一个 `Tools` 集合，
通过 `RegistrationId` 与 guard 精确管理任务生命周期；请求结构和集合接口见 core 文档。
跨类别 alias 协调与资源地址解析由调用入口承担。

## 接口导航

- [core 使用说明](core/README.md)：同步读取、集合与注册对象。
- [plugin 使用说明](plugin/README.md)：安装前提、导入请求与完成语义。
- [设计对应](DESIGN.md)：领域结构及函数行为。
- [读取与集合测试](core/tests/resources.rs)、[异步导入测试](plugin/tests/imports.rs)：真实文件和 ECS 调用流程。
