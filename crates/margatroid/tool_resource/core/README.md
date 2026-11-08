# tool_resource_core

独立领域 core，只依赖 `core_plugin` 和通用库；不依赖 address、其他领域 core 或 plugin。

`Tools` 以私有 `BTreeMap<String, ToolEntry>` 保存 alias 到完整工具记录，提供 Default、new、get、insert、insert_batch、remove_owner、iter。ToolEntry公开path、description、input_schema、owner_task_id和binding；InjectedToolDefinition公开alias、description、input_schema和binding，供宿主填入路径和任务归属。insert拒绝重复alias；insert_batch预先校验全部alias、路径、描述及schema，失败不修改集合；remove_owner仅移除指定任务拥有的条目。`WorldToolResourceExt::tool_entry` 借用完整记录，`tool_path` 借用同一记录的path，校验完整 Entity 和 alias，不访问文件系统。各领域只检查自身 alias；跨类别冲突由调用入口协调。

`read_tool(path)` 是不额外限制文件字节数的同步读取函数，供使用方在 blocking worker 调用。该函数用于导入校验，返回描述及schema后由Tools保存；已返回的临时材料不受之后文件变化影响。读取入口 TOML、同目录 `input.schema.json` 与 `main.lua`，要求 `schema_version = 1`、非空 description、合法 JSON 对象 schema 和 UTF-8 Lua 文本。允许内部 schema 引用，禁止外部引用；返回校验后的工具定义及程序文本，不执行 Lua。执行方按记录path只读取main.lua，不重读清单或schema；文件metadata变化不会隐式改写Tools记录。

读取入口通过 canonicalize 定位真实普通文件。相邻依赖的真实路径必须仍在入口父目录内。完整读取各文件并校验 UTF-8；分别返回 FileMissing、InvalidPath、InvalidUtf8、InvalidDefinition、ReadFailed，错误携带实际路径，解析失败不回显文件正文。调用方负责文件使用权限及使用时路径策略。

`ImportTool` 是携带完整 owner、alias、入口 path、CancellationToken 和共享 oneshot 回复的事件。core 只定义协议；异步校验和发布集合条目由 plugin 负责。同一请求的克隆共享一个可认领的回复端。

`ToolResourceError` 公开 kind、path、message，提供 new 和 with_path。临时输入类型定义在 types 模块并于 crate 根导出。测试全部在 tests 目录，覆盖集合更新、Entity generation、回复唯一性、按次读取和边界校验。
