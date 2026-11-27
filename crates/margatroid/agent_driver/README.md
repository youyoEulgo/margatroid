# AgentDriver

内置assistant的普通推理显式传递turn（缺失时为null）及执行tag=inference；所有压缩推理（手动、usage触发及请求体超限恢复摘要）显式调用 `EFFECT inference (msg.instructions, msg.compact_context, msg.compact_prompt) ? ?`，参数为 `{margatroid.data.null, "compact"}`，表示无turn的自由调用并带本次执行tag=compact。tag是推理执行关联，不写进AgentMessage或历史payload；成功摘要的type仍由Driver改为compact，snapshot及会话替换规则不变。

内置assistant的压缩分为判断与执行：maybe_compact(message)只读配置和上下文并返回boolean。compact()准备TEMP压缩输入、执行自由推理并必然返回一条AgentMessage：成功保留assistant字段、type改为compact；推理失败保留error；无早期上下文返回error说明。compact内部不写历史、不快照、不替换session.conversation。调用方process_compaction(message)先沿原history_append记录返回消息，error立即结束；只有完整且有效的compact才按原流程先快照再替换会话上下文。每次压缩推理前重新INJECT compact刷新压缩提示词；成功完成上下文替换后重新INJECT soul刷新系统提示词，失败不刷新系统提示词。

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

UI监控独立于查询，在Agent创建时传入的UI目录来源安装时建立，替换或停止时取消并等待退出，不由DTO每帧扫描维护。notify捕获变化，100ms安静窗口合并编译写入，双重扫描确认稳定后发布完整集合。core仅提供通用文件观察和owned状态，DTO负责网络类型校验及ClientSender筛选，不设置32MiB整体信封上限。初始化、错误及恢复都显式形成状态。

查询读取已发布观察状态，不启动监控或改写Agent。观察退出等待真实文件worker结束，daemon关停通过现有AxumLifetime跟踪完整监控Future。UI变化只影响资源交付，Agent程序是否重启按领域生命周期处理。

当前同步调度为：DriverUiWatch仅在稳定初值或真实内容/错误变化时调用DTO安装的UI状态回调，回调使用同一Clients槽位直接投递。相同内容和错误不重复发布，不调用World、RuntimeHandle或ECS同步事件；改造已在当前工作树实现。


当前工作树已实现统一Lua：复用既有资源挂载、Storage、MCL队列、任务监督、进程回收及同步入口。Workspace先启动独立Lua任务；Agent创建由公共Lua注入能力提交StartAgent，Agent不再启动/持有/终止Lua VM。执行者与目标分开，Agent统一归属Workspace，创建来源由实际任务绑定记录，配置按明确初值文件初始化持久化，UI按创建时传入目录直接监控，模型通过EFFECT model_set配置。公共mcl.execute指定目标，wait(agent)登记本次唤醒句柄；关闭Agent使用margatroid.agent:stop(address)注入方法提交原StopAgent并外部监督，不经过MCL。EFFECT tool_call只接收调用数组。Driver专用manifest依赖裁剪取消；工具发现元数据保持。内置Driver子代理资源已实现，前端源码不在本轮修改范围内。

程序资源不再要求driver.toml：Driver locator解析base.lua，read_driver只读取Lua和可选setting.toml。model参数放配置并经EFFECT model_set设置；导入由实际Lua显式执行。创建Agent时配置初值和UI只初始化一次，已有持久配置和ui保留。

内置assistant只在command封装检查一次顶层MCL返回值：Error以原message抛出并直接结束执行，其他结果原样返回。初始化、消息处理、压缩、历史和回合流程保持顺序业务代码，不用pcall掩盖MCL错误，不重试清理；宿主统一诊断和完成实际执行清理。Inference值内的RequestBodyTooLarge是可恢复领域结果，由下述Driver分支压缩并有限重推理，不等同于顶层MCL异常。底层MCL宿主API仍返回Error对象，其他Lua应用可以自行决定处理方式。

四态契约：Agent/Workspace公开starting、running、error、stopping；Agent挂载完成不是ready，MCL允许starting/running，推理和工具显式携带turn时要求running且匹配，省略时为Starting/Running自由调用。Lua创建的Agent在返回信息前绑定创建者真实任务scope与AgentSession；return_result不结束生命周期，任务取消/退出会提交原StopAgent并等待全部绑定Agent实际移除后才确认任务清理。主动停止Agent会解绑；异常日志沿原Agent存储处理，正常结束和取消不误报故障。关闭使用Lua agent.stop注入，不提供MCL agent_stop。

Agent创建注入接口为name/config_path/ui_path，拒绝旧resource_dir创建参数。配置仅首次从config_path初始化至Agent持久setting.toml，后续重启保留已修改配置；ui_path始终直接引用与监控，不创建Agent本地UI，不复制、覆盖、删除已有UI副本，也不把旧副本作为回退来源。Lua程序加载只读取程序文件，不推导或读取同目录配置。

Driver任务目标保存在自身配置driver_task={agent=执行地址,task_id=实际任务id}，每次初始化更新。已有task_send工具的task_id支持该结构化引用，发送value={type="compact"}即可主动压缩，不建立turn；成功仅表示入队。压缩仍按Driver串行执行流进行，不抢占当前推理或工具，处理完当前输入后再接收邮箱；没有早期上下文时不做替换。

推理领域执行失败以Inference.message={type="error",code=领域错误码,message=原错误文本,request_id=原请求id}及stop_reason="error"返回，不走顶层MCL Error。RequestBodyTooLarge及恢复过程中的同类错误、无进展诊断通过margatroid:log("warn", message, {code,request_id,details})进入实际Agent日志，不追加消息历史或子任务输出；只有该code进入Driver请求体恢复。其他错误继续记录消息历史，清pending_tool_results后按原路径finish_turn。手动及usage触发的压缩保留原行为；MCL参数/权限/队列或历史写入错误仍由原command封装处理。

请求体恢复读取当前conversation，按完整工具调用/结果批次枚举较早前缀，优先保留compaction.keep_messages尾部。摘要请求本身若RequestBodyTooLarge，就二分缩短到更小的完整前缀，不重复同一候选；旧checkpoint无法再缩小时则尝试更大的未压缩平衡前缀，可涵盖部分原保留尾部但至少保留最后消息。每次恢复最多16次不同候选尝试。没有可压缩前缀、最小批次仍超限、摘要未正常完成或所有可用候选均不能实质减小时明确记录失败并结束turn。正文大小是转换后的HTTP字节数，Driver用领域conversation编码大小只做进展证明，不冒充HTTP大小估算。

摘要成功后，先复核conversation未被外部更改，再记录compact历史、snapshot原session，最后以checkpoint消息加未压缩尾部替换实时conversation；不删除或改写原历史/图片。精确使用本次候选前缀的边界，不重新计算split。恢复后在同一turn重新推理，每个turn最多8次恢复，防止尾部、工具定义或系统提示本身超限引发无限循环。后续重推理由上游实际受理结果决定是否继续恢复，不使用本地配置大小预检。

内置Driver的初始设置包含[initialization]：default为原静态IMPORT/CREATE/INJECT指令对象数组，每项固定为{command=指令文本,arguments=参数数组}，custom初始为[]，供用户追加初始化指令。initialize读取Agent配置，严格按default再custom顺序逐条command(item.command,item.arguments)执行；参数化model_set、sandbox_use、driver_task登记及最后ready保留在脚本，任一指令失败不发送ready。initialization、default或custom缺失或为空时跳过对应数组，不再读取资源配置补齐，不合并隐藏指令。修改custom在下一次Driver初始化时生效，编辑保留默认数组和soul/compact及msg/entry/session等运行依赖。

初始化default/custom统一使用对象条目，不混用旧字符串；无参数条目显式arguments=[]，有参数条目以arguments按顺序绑定?。参数保留TOML可表示的数组/对象/数字/布尔/字符串类型，不做文本替换或内联JSON拼接；tool_call对象内的arguments沿原协议为JSON字符串。现有用户持久设置不自动迁移；已有initialization段中的字符串条目需手动改为对象。

内置assistant的base.lua与main.lua保持相同程序，按instructions参数是否存在区分主Driver与子代理。input.schema.json只校验默认工具启动的node_name、alias、path，不包含instructions；主模式读取subagent.schema.json原样注册subagent，该Schema只开放必需instructions与可选path，不开放node_name或alias。子模式不注册subagent工具。

subagent采用全新对话：config_path固定资源目录setting.toml，不读取父Agent配置/上下文，不继承父initialization.custom或动态工具，不注入继承边界或父消息。默认配置初始化完成并ready后return_result({task_id})，再将委托instructions作为第一条user消息正常记录历史、开始子回合。只有Driver默认配置中定义的工具/初始化可用于子代理。

子模式与主模式共用history_append记录本轮user/assistant/tool_call/error/compact及真实sequence。任务输出包含非空assistant.content，error额外追加[subagent error]运行时诊断；reasoning、工具结果、user及compact不写任务输出。子Agent结束回合后只清空turn并进入task.receive等待，不调用agent.stop或自行finish；Agent与任务输出一起保留，父Agent通过task_read读取后显式task_finish才关闭任务和子Agent；宿主异常沿既有task.error及真实清理路径交付，不自动重试。

主Agent创建时ui_path使用资源目录/ui，子模式使用/subagent-ui。两种UI来源均由原目录监控和agent.ui接口交付，不复制、不回退主UI。创建来源creator由真实任务绑定产生，不由脚本传owner。

skill_register与mcp_register在实际注册成功后，使用目标Agent CONFIG GET/SET向initialization.custom追加{command="EFFECT tool_call ?",arguments=[[{id="startup-<alias>",tool_name=<实际注册工具alias>,arguments=<注册参数JSON>}]]}。skill保存规范SKILL.md路径，MCP保存server及alias_prefix，不保存当次task_id、发现结果或进程句柄。首次注册及启动重放如已存在完全相同命令参数则不追加、不SET，现有活跃alias冲突仍报错；不同的首次注册仍按追加处理。

MCP注册批次同时加入prefix+update工具。无binding、mcp_tool、mcp_update分别进入首次注册、远端调用和更新分支；标记与任务关联来自宿主注入binding，不进入模型Schema。更新调用任务独立于旧服务，先校验新参数和唯一初始化目标，再finish旧任务并等待实际清理，当前任务直接接替建立新连接和注入工具，不嵌套执行注册工具。成功后只替换原初始化条目，保留其他条目及顺序；保存的入口仍为原普通注册工具及新参数，不保存更新工具别名或任务id。

CONFIG写入失败返回“本次已注册但启动命令未保存”的失败说明。skill静态工具保留；MCP返回失败说明及当前serving task_id，任务仍存活、维持已注册工具转发。后续启动重新执行已保存命令，建立新MCP连接/任务，不使用旧task_id。GET编辑SET之间不做隔离，整数组覆盖语义由调用方接受。

持久配置是本次初始化的唯一来源；缺失或空初始化不回退资源模板。资源setting.toml只用于新Agent配置文件首次创建，子代理不继承父持久注册项。


内置Driver现统一为 `driver:margatroid/assistant:dev`，默认提示词为 `prompt:margatroid/assistant-soul:dev`，定位为通用助手。reviewer及旧coder资源不再提供，不保留旧资源路径兼容入口。Driver的base.lua与main.lua均包含RequestBodyTooLarge恢复；实际运行任务只在重新启动后加载新程序。既有用户数据库、Agent节点、历史及持久配置不自动改名，原tool_path指向旧资源的记录需明确更新后才能再次Existing启动。
