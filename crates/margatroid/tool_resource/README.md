# Tool Resource

tool_resource_core定义工具路径集合、文件读取方法、导入事件和错误，只依赖mecs及通用库。tool_resource_plugin负责异步校验和路径登记，不依赖其他领域plugin，也不让core依赖address或其他领域core。

## 导入与路径

调用方plugin校验资源种类、Driver依赖和跨类别alias冲突，通过address定位tool.toml后发送ImportTool。本插件校验文件，成功只登记Tools中的alias与规范路径，不保存来源定位字符串或正文。

MCL对同一目标的IMPORT串行受理，跨类别alias唯一性由MCL入口协调；各资源core只管理自己的集合，不新增通用绑定表或资源注册表。取消、目标失效或校验失败保留旧路径；发布成功后才通过原oneshot回复，不发送导入结果观察事件。

## 按次读取

每次使用从登记路径读取tool.toml、input.schema.json和main.lua。校验清单版本、非空描述、JSON对象schema及Lua文件UTF-8；schema拒绝外部引用；不执行Lua。读取发生在blocking worker，使用方等待真实完成，不阻塞ECS线程。导入时校验不保证日后文件仍存在或合法；使用时重新校验，失败不退回旧副本。

工具描述和schema在准备推理请求时读取；程序在实际执行调用时重新读取。本次推理定义与之后磁盘内容不保证相同版本。工具执行方重新校验实际参数，不把早先模型看到的schema当作当前执行依据。

## 职责

本领域只登记路径、解析工具文件；进程、Lua运行、沙箱和工具调用结果归tool执行领域。集合只保存BTreeMap<String, PathBuf>，不使用ResourceID、LibraryComponent或通用ResourceSet；加载后的临时值不再携带资源身份。路径用于打开文件，不代替沙箱与文件访问检查。

异步校验直接返回Result<ImportOutput, ImportError>，System只路由，Handler负责发布及oneshot回复。所有结果保留原owner Entity，迟到结果不写入复用index的新实体。文件读取失败报告alias、实际路径和底层原因，不回显文件正文。
