# DtoPlugin

DtoPlugin适配WebSocket协议与领域事件。它依赖protocol、address、各领域core和mecs，不依赖其他plugin，不单独提供dto_core。

## 入站

接收消息后按protocol解析信封。除连接注册外，先从真实连接取得Client Entity，拒绝未注册连接和同一Client重复的在途请求ID。运行目标通过address解析为完整Entity，再检查组件、Workspace归属和实例。未运行的Workspace直接从启动时加载的内存配置选择，不创建占位Entity。

列出Workspace同步读取内存配置并回复，不发送领域事件或提交异步任务；配置文件变化由Margatroid统一重载。异步请求生成独立operation_id，记录原网络ID、Client Entity、连接及目标实例，再转换成领域请求。不同连接可以使用相同网络ID；内部事件不传播外部实体地址。

## 回执与同步

领域结果按operation_id关联，复检原目标、Client实例和连接后，只向原连接发送回执并释放关联。断连删除网络关联，不撤销已受理领域操作；迟到结果不绑定新连接或同地址新实体。

状态同步只读取领域组件和已加载的内存配置，通过Address取得对外地址。缓存保存最近投影，不建立第二份领域状态。消息不包含凭据、源码、数据库句柄或内部Entity表示。

## 职责边界

Protocol只定义网络类型、序列化和格式校验；依赖领域类型的转换、寻址和错误映射均在DtoPlugin。Handler负责具体边界处理，System只收集事件并路由。

DtoPlugin不执行MCL、模型推理、资源加载、数据库操作、目录扫描或生命周期清理。历史读取通过领域能力异步完成。请求受理、持久提交和运行成功分别表达，不把其中一个当作另一个的证明。

入站消息总上限32MiB，请求ID非空且最多512个UTF-8字节。有可用ID的非法请求返回request.failed；无法关联的非法消息关闭连接，格式错误使用1007，超大消息使用1009。错误不回显凭据、图片或请求正文。
