# AgentDriver

内置coder/reviewer的压缩分为判断与执行：maybe_compact(message)只读配置和上下文并返回boolean。compact()准备TEMP压缩输入、执行自由推理并必然返回一条AgentMessage：成功保留assistant字段、type改为compact；推理失败保留error；无早期上下文返回error说明。compact内部不写历史、不快照、不替换session.conversation。调用方process_compaction(message)先沿原history_append记录返回消息，error立即结束；只有完整且有效的compact才按原流程先快照再替换会话上下文。每次压缩推理前重新INJECT compact刷新压缩提示词；成功完成上下文替换后重新INJECT soul刷新系统提示词，失败不刷新系统提示词。

本轮共享Client槽位、统一写入口与异步监控已实现，尚未提交。具体函数和边界见本领域DESIGN及[DTO说明](../dto/README.md)；运行进程需使用新构建。
agent_driver_core提供Driver文件格式、读取函数及错误，使用标准库、serde/TOML、notify、SHA-256、MIME及Tokio等通用库。Agent启动闭包直接调用，不需要Driver加载Plugin、System、请求事件或oneshot中转。

## 文件

```text
Driver目录
├── base.lua
├── setting.toml
└── ui/
    ├── index.html
    └── assets/
```

三个文件均必需，默认setting.toml允许为空。清单声明schema_version、dependencies字符串数组及inference模型参数，使用Driver自己的类型，不依赖其他领域core。

## 启动读取

Workspace将请求定义中的成员启动参数交给StartAgent。Agent启动闭包用Driver定位字符串调用address::resolve_file取得清单路径，再通过agent_driver_core::read_driver读取文件；同步文件读取在blocking worker执行，不阻塞ECS线程。

Agent持有本次实际加载来源的AgentDriverSource，保存Driver地址及清单规范路径，UI观察以此定位。重启完成旧Driver清理时移除原来源，读取新Driver后写入本次来源；运行中的Lua程序仍按既有VM生命周期执行。

## 设置与消息

Agent数据库不存在就创建；Agent setting.toml不存在就从Driver默认模板创建，已有文件不覆盖、不合并，格式损坏直接报错。初始化由Agent启动流程协调，不由Driver读取函数写Agent数据。

Block只保存消息数组，CREATE BLOCK不写MESSAGE类型，存在则打开且不清空；设置使用CONFIG GET、CONFIG SET ? ?、CONFIG DELETE ?。运行指令使用EFFECT，无EMIT前缀。

工具、提示词和沙箱仍由所属领域维护alias到路径的集合；它们在运行期间反复访问，与仅启动时读取一次的Driver不同。

## Driver UI

Driver UI读取及notify观察已实现。agent.ui.req/res/sync经DTO使用这些core能力；ui目录缺失的Driver仍正常运行。

ui目录提供原生Web或Vue/React等编译产物。后端读取完整静态文件集合，计算内容revision，所有文件通过base64随WS交付。ui目录缺失表示该Driver没有UI；存在但index.html缺失表示资源错误。前端负责资源URL映射及中央页面加载，平台保留左右侧栏和顶部栏。

UI监控独立于查询，在Agent创建时传入的UI目录来源安装时建立，替换或停止时取消并等待退出，不由DTO每帧扫描维护。notify捕获变化，100ms安静窗口合并编译写入，双重扫描确认稳定后发布完整集合。core仅提供通用文件观察和owned状态，DTO负责网络类型、32MiB信封预算及ClientSender筛选。初始化、错误及恢复都显式形成状态。

查询读取已发布观察状态，不启动监控或改写Agent。观察退出等待真实文件worker结束，daemon关停通过现有AxumLifetime跟踪完整监控Future。UI变化只影响资源交付，Agent程序是否重启按领域生命周期处理。

当前同步调度为：DriverUiWatch仅在稳定初值或真实内容/错误变化时调用DTO安装的UI状态回调，回调使用同一Clients槽位直接投递。相同内容和错误不重复发布，不调用World、RuntimeHandle或ECS同步事件；改造已在当前工作树实现。


当前工作树已实现统一Lua：复用既有资源挂载、Storage、MCL队列、任务监督、进程回收及同步入口。Workspace先启动独立Lua任务；Agent创建由公共Lua注入能力提交StartAgent，Agent不再启动/持有/终止Lua VM。执行者与目标分开，Agent归属Workspace或父Agent，配置按明确初值文件初始化持久化，UI按创建时传入目录直接监控，模型通过EFFECT model_set配置。公共mcl.execute指定目标，wait(agent)登记本次唤醒句柄；关闭Agent使用margatroid.agent:stop(address)注入方法提交原StopAgent并外部监督，不经过MCL。EFFECT tool_call只接收调用数组。Driver专用manifest依赖裁剪取消；工具发现元数据保持。初始化/委托/子代理应用未实现；前端源码不在本轮修改范围内。

程序资源不再要求driver.toml：Driver locator解析base.lua，read_driver只读取Lua和可选setting.toml。model参数放配置并经EFFECT model_set设置；导入由实际Lua显式执行。创建Agent时配置初值和UI只初始化一次，已有持久配置和ui保留。

内置coder/reviewer只在command封装检查一次MCL返回值：Error以原message抛出并直接结束执行，其他结果原样返回。初始化、消息处理、压缩、历史和回合流程保持顺序业务代码，不逐次传播错误，不使用pcall/recover，不补写错误会话或重试收尾；宿主统一诊断和完成实际执行清理。底层MCL宿主API仍返回Error对象，其他Lua应用可以自行决定处理方式。

四态契约：Agent/Workspace公开starting、running、error、stopping；Agent挂载完成不是ready，MCL允许starting/running，推理和工具显式携带turn时要求running且匹配，省略时为Starting/Running自由调用。Lua创建的Agent在返回信息前绑定实际执行实例与AgentSession监控；执行异常真实收口后写Agent日志，原提交探针直投Clients，再通过AgentException发布error，正常结束和取消不误报故障，旧会话解绑。关闭使用Lua agent.stop注入，不提供MCL agent_stop。

Agent创建注入接口为owner/name/config_path/ui_path，拒绝旧resource_dir创建参数。配置仅首次从config_path初始化至Agent持久setting.toml，后续重启保留已修改配置；ui_path始终直接引用与监控，不创建Agent本地UI，不复制、覆盖、删除已有UI副本，也不把旧副本作为回退来源。Lua程序加载只读取程序文件，不推导或读取同目录配置。

Driver任务目标保存在自身配置driver_task={agent=执行地址,task_id=实际任务id}，每次初始化更新。已有task_send工具的task_id支持该结构化引用，发送value={type="compact"}即可主动压缩，不建立turn；成功仅表示入队。压缩仍按Driver串行执行流进行，不抢占当前推理或工具，处理完当前输入后再接收邮箱；没有早期上下文时不做替换。

推理领域执行失败以Inference.message={type="error",message=原错误文本,request_id=原请求id}及stop_reason="error"返回，不走顶层MCL Error。普通回合将error排入原消息入口，记录历史并等待本轮真实清理；压缩调用直接记录error，不改成compact、不替换上下文、不自动重试，Driver仍可接收后续任务。MCL参数/权限/队列或历史写入等真实执行错误仍由原command封装处理。

内置Driver的初始设置包含[initialization]：default为原静态IMPORT/CREATE/INJECT指令文本数组，custom初始为[]，供用户追加初始化指令。initialize读取Agent配置，严格按default再custom顺序逐条command执行；参数化model_set、sandbox_use、driver_task登记及最后ready保留在脚本，任一指令失败不发送ready。已有Agent持久配置缺少整个initialization段时，只读Driver资源目录setting.toml中的该段执行，不自动写回或覆盖持久配置；显式配置了该段时使用其default/custom，不合并额外隐藏指令。修改custom在下一次Driver初始化时生效，编辑保留默认数组和soul/compact及msg/entry/session等运行依赖。
