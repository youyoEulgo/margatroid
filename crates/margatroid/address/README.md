# address

address提供实体对外寻址和磁盘文件定位，不提供Plugin、图书馆组件、通用资源集合、类型注册表或资源生命周期管理。

## 依赖

address位于core层，只依赖mecs、标准库及通用基础库，不依赖其他领域core或plugin。领域plugin使用address解析外部输入；其他领域core不依赖address。

## 实体寻址

Address作为独立组件挂载到需要公开寻址的Entity。实体地址使用完整绝对节点路径，例如/、/demo、/demo/coder/reviewer；按大小写原样保存。拒绝相对路径、空段、尾部斜杠、.或..段、反斜杠、冒号和控制字符，返回InvalidNodePath。不会补齐路径、创建父实体、推导所有权或映射为磁盘目录。World扩展只提供按地址查找Entity和地址查重；不维护第二份索引。挂载或改名的调用方在同一ECS修改流程中先查重，再写组件。

外部地址在入口解析一次，内部组件关联、事件和Handler使用完整Entity。异步操作保留解析时的Entity，不按旧地址重新寻找替代实例。对外响应需要地址时读取Address组件。

## 文件定位

资源字符串使用type:scope/name:tag，必须显式提供非空tag，不补齐默认值。文件定位函数将字符串解析为真实文件路径；磁盘布局和资源入口由各资源领域约定。实体地址查询和文件定位互不兜底。

提示词和沙箱导入后登记alias与规范入口路径；工具集合另外保留description和input_schema等工具定义，实际执行仍按路径读取程序。各集合由所属core定义，不在address建立统一资源注册表。

## 随用随取

每次实际打开资源从登记路径读取；工具执行使用登记时固定的Schema和描述，不隐式重导入metadata，其余资源按所属领域契约读取；文件删除、权限变化或格式错误直接返回失败，不回退旧内容。实际打开文件时仍检查路径和执行权限。

一次推理、工具执行、沙箱执行或Driver启动可持有本次所需的临时数据，文件变化不修改在途操作。Driver在每次启动时读取程序，不在运行中热替换。显式写入对话Block的内容属于对话数据，不是文件资源缓存。

resolve_file的root为对应类型根目录：driver使用agent-drivers，tool使用tools，prompt使用prompts，sandbox使用sandboxes；函数不读取HOME或创建目录。

实体路径与文件资源解析分别由Address和ParsedAddress负责，不互相兜底。该说明对应当前源码；运行中的daemon是否使用此实现由部署版本决定。域内测试分别覆盖节点校验/完整Entity查找和文件资源tag/真实文件边界。

## 资源查询

`query_resources` 在传入的 `ResourceRoots` 下按 `type`、`scope`、`name` 精确组合筛选，省略表示不限，返回全部 tag，按类型／作用域／名称／标签稳定排序。调用方在 blocking worker 执行一次扫描，函数不建立缓存或监控、不读取程序正文、不创建目录。不存在的类型根返回空列表，读取失败明确返回错误。

结果保留 `type:scope/name:tag` 地址及 `resolve_file` 的实际入口：Driver 为 `base.lua`，工具为 `tool.toml`，提示词为 `prompt.toml`，沙箱为 `policy.json`。另以可空的 `tool_path` 提供工具启动入口：工具等于其入口，Driver 存在同目录工具清单时指向该清单，其余为 null。这样前端不用推导文件布局，也不会将 Lua 文件当作工具清单启动。
