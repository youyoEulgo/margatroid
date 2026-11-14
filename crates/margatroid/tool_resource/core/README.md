# tool_resource_core

独立领域 core，只依赖 `core_plugin` 和通用库；不依赖 address、其他领域 core 或 plugin。

`Tools`使用私有entries和registrations保存公开定义与唯一生命周期凭证，不可Clone。new要求同步UpdateProbe，get/iter过滤已关闭作用域，insert/insert_batch/insert_registered_batch共用整批校验和提交入口；remove按alias删除，revoke按alias及RegistrationId精确撤销，不再按owner扫描。目标集合销毁或主动删除时，guard释放并解除任务侧登记。TaskScope由tool_core持有，跨领域只用类型擦除的guard和Weak可用性回调，不建立tool_resource_core到tool_core的依赖。

ToolEntry增加可信owner_agent，与owner_task_id共同描述生命周期任务；二者只由宿主实际解析lifetime得到。Lua RegisterToolsRequest使用agent和tools，InjectedToolDefinition使用alias、description、input_schema、可选lifetime及binding；旧owner_task_id输入明确拒绝。WorldToolResourceExt保持按完整Entity/alias只读借用，不访问文件系统。

`read_tool(path)` 是不额外限制文件字节数的同步读取函数，供使用方在 blocking worker 调用。该函数用于导入校验，返回描述及schema后由Tools保存；已返回的临时材料不受之后文件变化影响。读取入口 TOML、同目录 `input.schema.json` 与 `main.lua`，要求 `schema_version = 1`、非空 description、合法 JSON 对象 schema 和 UTF-8 Lua 文本。允许内部 schema 引用，禁止外部引用；返回校验后的工具定义及程序文本，不执行 Lua。执行方按记录path只读取main.lua，不重读清单或schema；文件metadata变化不会隐式改写Tools记录。

读取入口通过 canonicalize 定位真实普通文件。相邻依赖的真实路径必须仍在入口父目录内。完整读取各文件并校验 UTF-8；分别返回 FileMissing、InvalidPath、InvalidUtf8、InvalidDefinition、ReadFailed，错误携带实际路径，解析失败不回显文件正文。调用方负责文件使用权限及使用时路径策略。

`ImportTool` 是携带完整 owner、alias、入口 path、CancellationToken 和共享 oneshot 回复的事件。core 只定义协议；异步校验和发布集合条目由 plugin 负责。同一请求的克隆共享一个可认领的回复端。

`ToolResourceError` 公开 kind、path、message，提供 new 和 with_path。临时输入类型定义在 types 模块并于 crate 根导出。测试全部在 tests 目录，覆盖集合更新、Entity generation、回复唯一性、按次读取和边界校验。
