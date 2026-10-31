# prompt_resource_core

独立领域 core，只依赖 `core_plugin` 和通用库；不依赖 address、其他领域 core 或 plugin。

`Prompts` 以私有 `BTreeMap<String, PathBuf>` 保存 alias 到真实入口文件路径，提供 Default、new、get、insert、remove、iter。路径读取扩展 `WorldPromptResourceExt::prompt_path` 仅借用集合条目，校验完整 Entity 和 alias，不访问文件系统。各领域只检查自身 alias；跨类别冲突由调用入口协调。

`read_prompt(path, limits)` 是同步有界读取函数，供使用方在 blocking worker 调用。每次调用重新读取和解析；不缓存正文，已返回的临时材料不受之后文件变化影响。读取入口 TOML 与同目录 `content.md`，清单 role 必须为 system、user 或 assistant，正文必须为非空 UTF-8。返回独立 `PromptRole`，不依赖消息领域或推测角色。

读取入口通过 canonicalize 定位真实普通文件。相邻依赖的真实路径必须仍在入口父目录内。文件大小在打开后及读取过程中检查；分别返回 FileMissing、InvalidPath、TooLarge、InvalidUtf8、InvalidDefinition、ReadFailed，错误携带实际路径，解析失败不回显文件正文。调用方负责文件使用权限及使用时路径策略。

`ImportPrompt` 是携带完整 owner、alias、入口 path、CancellationToken 和共享 oneshot 回复的事件。core 只定义协议；异步校验和发布集合条目由 plugin 负责。同一请求的克隆共享一个可认领的回复端。

`PromptResourceError` 公开 kind、path、message，提供 new 和 with_path；`PromptReadLimits` 提供 Default，可按文件分别指定 usize 字节预算。临时输入类型与预算定义在 types 模块并于 crate 根导出。测试全部在 tests 目录，覆盖集合更新、Entity generation、回复唯一性、按次读取和边界校验。
