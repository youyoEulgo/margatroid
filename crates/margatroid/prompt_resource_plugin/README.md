# prompt_resource_plugin

已实现的提示资源加载与主线程挂载插件，不是资源执行器。

## 安装与根目录

```rust
app.add_plugin(RuntimePlugin::default())
    .add_plugin(ClosurePlugin)
    .add_closure_system(RuntimePlugin::PRE_UPDATE)
    .add_plugin(AsyncRuntimePlugin)
    .add_plugin(ResourcePlugin)
    .add_plugin(PromptResourcePlugin::open(global_root)?);
```

`open(global_root)` 接受固定全局根目录，资源位于 `global_root/prompts/scope/name/tag`，默认计划是 PRE_UPDATE。可用 `with_schedule` 与 `with_limits` 配置；自定义计划必须已安装 ClosureSystem。本插件不创建资源包、不提供项目或Driver包内回退。

**Driver 根是另一种约定：** AgentDriverLoaderPlugin 接受独立 agent-drivers 根，直接拼 `scope/name/tag`；不要将已经包含 `prompts` 的目录传给本插件，也不要把 Driver 根解释为资源全局根。

## 共享 API

条目是 `margatroid_types::resource_components::PromptResource`，实际定义在组合文件 `types/src/resource_components.rs`。加载事件与公共错误均是 `margatroid_types::resource_events` 中的类型，实际定义在组合文件 `types/src/resource_events.rs`，插件没有本地 events 文件。

发送 `LoadPromptResource { id, target, resource_id, alias, import_kind, cancel, reply, activity }`。`reply` 是 `Arc<Mutex<Option<oneshot::Sender<Result<(), ResourceLoadError>>>>>`；请求可 Clone，但 Sender 不可 Clone。`activity` 为 `Option<Arc<ExecutionActivity>>`，MCL移交已登记的 Other 活动，加载器不重复登记。

等待 reply 表示主线程已完成集合挂载或失败收口；`LoadPromptResourceResult` 仅供观察。集合是目标 Entity 上的 `ResourceSet<PromptResource>`，身份仅存在集合键中，本地 alias 不改变 ResourceID。

## 明确类型导入与普通加载

`import_kind: None` 是普通加载，允许Agent或非Agent目标，只挂载集合、不发布binding；MCL IMPORT传 `Some(InnerType::Message)`。加载插件在提交和挂载前使用共享 `validate_resource_import` 校验声明与地址、完整Agent实例、精确driver_dependencies许可及跨类别alias冲突。已有alias只能同ID同kind重载；同ID改名先查冲突，成功时移除旧名。

按PromptResource.role构造ImportedValue::Message的Arc<[AgentMessage]>；system对应System、user对应User、assistant对应无tool_calls的Assistant，正文来自已读取content，不能将System降级为User。

共享 `mount_resource_import` 在同一个受检主线程步骤内复检许可、准备按ID键替换的ResourceSet、借用原registry检查集合类型和全部地址，然后绑定并受检发布集合。发布失败不修改集合，并精确恢复旧ImportBindings。读取、校验或取消失败均保留旧条目和binding；None从不修改binding。成功重载后新GET取得新值，旧Block仍持有旧快照；IMPORT不自动注入、开放模型工具或激活沙箱。

## 读取、监督与发布

默认单文件上限：definition/schema为1MiB，content/script/policy各8MiB；未使用的字段不添加额外文件要求。读取先检查长度，再逐8KiB块核对累计上限、取消及UTF-8。

读取prompt.toml中的role，要求system、user或assistant；有界读取content.md，要求有效UTF-8且非空。作用域不编码角色。保存内存正文。

一个领域 System 使用 `send_async_closure` 调度读取；`spawn_blocking`只读准备工作，独立 supervisor 等待 JoinHandle 真正退出才交主线程结果。未开始提交失败 rollback，已开始工作在主线程收口后 mark_completed。取消须等待收口，不把发送取消或丢弃 Receiver 当作 I/O退出证明。

必要路径与祖先拒绝 symlink 和父级跳转，文件要求普通文件、有界 UTF-8、非空。非必要目录项忽略。路径检查不是对恶意并发替换目录的原子快照保证。

重复加载重新读取，成功仅替换一个 ID键，普通读取失败保留旧条目和alias。挂载前再次检查取消与完整 Entity generation，禁止迟到结果写入复用index。reply已take不重复挂载，但仍完成活动收口。Some导入同时检查binding冲突，不按加载顺序任取资源；取消后禁止挂载及binding发布。

## 验证

集成测试覆盖成功、失败保留、取消无挂载、generation复用、alias更新、目标隔离、其他key保持、重复结果、读取限长、UTF-8及symlink。五插件联合回归还覆盖错误类型与alias冲突保旧、typed GET、改名、旧Block快照、Prompt角色忠实及Sandbox纯重载不改激活。CallableResource只冻结元数据和文件路径，不承诺脚本/正文文件内容版本或多文件原子快照。完整字段、错误与生命周期边界见同目录 DESIGN。

## 可操作的加载诊断

加载失败必须保留完整 ResourceID、实际尝试的绝对文件或目录路径，以及底层 I/O、UTF-8、清单解析或校验原因；不得用笼统的“读取失败”覆盖原因，也不得在添加上下文时截断路径或原因。逐祖先检查失败同时保留最终尝试路径和失败的祖先路径。

依赖加载器在原有错误外添加资源上下文，保留上层调用方已经提供的依赖链（包括请求 id 中的关联链）；本插件不凭空推断驱动来源，也不递归加载依赖。调用方应将 `driver -> dependency` 链追加在本错误之外；Driver 清单声明的依赖仍只解析地址，不检查存在性。缺失依赖由对应 tool、prompt 或 sandbox 加载器报告其自身完整地址及实际绝对路径。

错误在异步读取、主线程挂载及业务回执之间传播时保留原分类和原因；诊断文本不再受插件的旧 512 字节/字符截断限制，公共错误类型不变。回归测试覆盖缺失驱动目录、缺失 driver.toml/base.lua，以及缺失各类依赖文件。
