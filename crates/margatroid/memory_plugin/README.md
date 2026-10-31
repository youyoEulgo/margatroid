# Storage

storage_core定义AgentStorage、Block、历史和设置操作及其文件实现，只依赖mecs、SQLite、TOML等通用库，不依赖其他领域core。storage_plugin取得组件句柄并调度blocking worker，通过原请求oneshot回复，不提供额外结果事件。

## 初始化与生命周期

Agent启动闭包在blocking worker调用open_storage。数据库不存在则创建，已有数据库保留内容并校验schema。Agent settings.toml不存在才使用Driver默认模板创建；数据库与设置分别判断。模板可为空，已有设置不覆盖、不合并，格式损坏直接报错。

AgentStorage独立挂载到Agent Entity，持有数据库句柄及设置路径，不嵌入Agent。克隆共享连接和文件锁，不缓存Block或设置正文。所有需要协调的同库操作共享连接管理器；停止关闭新受理并等待真实worker退出后再释放句柄，不因丢弃Future认定操作已结束。

storage_core只认识路径和存储操作，不读取Workspace配置、定位Driver、加载工具或管理VM。文件路径由Agent启动参数明确提供，配置指令不允许改成任意文件路径。

## 消息Block

普通Block和历史保存在同一Agent SQLite文件中，不使用ResourceID作为运行索引或持久Block命名空间。持久Block按库内名称识别；TEMP额外使用跨进程唯一的VM实例标识隔离，重启不恢复旧TEMP。

Block字段只保存结构化消息数组，不保存TOOL、SANDBOX、资源地址或alias绑定。消息结构由调用方按消息领域契约校验，存储层保留结构化值，不依赖消息core，也不根据字段名推测用途。

CREATE字段名不带类型，不存在则创建，存在且字段及生命周期一致则打开；不清空消息，不返回created标志，冲突不重建数据。

多选择器GET在同一读事务中取得快照。INJECT的所有数据库来源、目标读取、范围检查和替换在同一写事务完成；自引用先读完再写。外部Prompt等文件内容由调用方读取后作为Literal传入，不在SQL事务中加载资源文件。

## 历史

历史只追加，按数据库序号读取。key不能为空；同key同事实重试不重复追加，不覆盖第一次写入时间；内容冲突使整批回滚。记录保存正文、来源、turn、元数据和真实时间；旧记录未知时间保留0，不猜测或回填。

消息历史由显式history_append写入，指令审计由MCL另行发起。存储不自动记录推理、工具或Block操作结果。Block修改和历史追加是独立事务，共用文件不代表共同提交。

## TOML设置

每次配置操作重新读取当前文件，不维护长期解析副本。同一设置文件的读改写串行执行。

CONFIG GET取得完整table。SET按点分路径新增或整体替换值，缺失中间table自动建立，已有非table中间节点返回错误。DELETE删除指定键，目标缺失视为成功，不清理空父table。路径不支持数组索引；不以nil或空数组表示删除。

修改先解析及校验，序列化完整新文件，写入同目录临时文件并同步，再原子替换目标及同步目录。替换前失败保留旧文件；替换已发生但持久性确认失败返回CommitUnknown，不谎称旧文件未变。临时文件失败不作为设置成功，不自动用默认模板恢复。

SQLite事务和TOML文件修改互相独立，不承诺跨文件事务；初始化一项成功、另一项失败时如实返回错误，重试不覆盖已存在数据。TOML键值保留语义，不承诺保留手工注释和排版。

## 取消与错误

取消前已提交的修改保持成功事实；只有确认未提交且清理完成才能报告Cancelled。无法确认提交事实返回CommitUnknown，不自动重做操作。

回滚或连接清理失败返回CleanupFailed，将共享连接标记不可用并拒绝后续IO。调用方在真实worker退出或明确清理失败后交付结果，不能提前通知Agent或Workspace已安全停止。设置操作同样等待真实文件worker，不靠丢弃Future中断写入。
