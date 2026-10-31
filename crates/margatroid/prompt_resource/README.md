# Prompt Resource

prompt_resource_core定义提示词路径集合、文件读取方法、导入事件和错误，只依赖mecs及通用库。prompt_resource_plugin负责异步校验和路径登记，不依赖其他领域plugin，也不让core依赖address或其他领域core。

## 导入与路径

调用方plugin校验资源种类、Driver依赖和跨类别alias冲突，通过address定位prompt.toml后发送ImportPrompt。本插件校验文件，成功只登记Prompts中的alias与规范路径，不保存来源定位字符串或正文。

MCL对同一目标的IMPORT串行受理，跨类别alias唯一性由MCL入口协调；各资源core只管理自己的集合，不新增通用绑定表或资源注册表。取消、目标失效或校验失败保留旧路径；发布成功后才通过原oneshot回复，不发送导入结果观察事件。

## 按次读取

每次使用从登记路径读取prompt.toml和content.md。校验role为system、user或assistant，正文为非空有界UTF-8；不按alias、scope或目标字段猜测角色。读取发生在blocking worker，使用方等待真实完成，不阻塞ECS线程。导入时校验不保证日后文件仍存在或合法；使用时重新校验，失败不退回旧副本。

GET或INJECT使用Prompt alias时重新读取。读取结果由使用方plugin转换为消息，保持System、User或Assistant角色；prompt core不依赖消息core。显式注入Block后的消息属于对话数据，文件变化不修改已写入消息。

## 职责

本领域不写Block、不追加历史、不提交推理。集合只保存BTreeMap<String, PathBuf>，不使用ResourceID、LibraryComponent或通用ResourceSet；加载后的临时值不再携带资源身份。路径用于打开文件，不代替沙箱与文件访问检查。

异步校验直接返回Result<ImportOutput, ImportError>，System只路由，Handler负责发布及oneshot回复。所有结果保留原owner Entity，迟到结果不写入复用index的新实体。文件读取失败报告alias、实际路径和底层原因，不回显文件正文。
