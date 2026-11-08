# Tool

tool_core定义ToolTasks、工具请求、结果和错误，只依赖mecs及通用库。tool_plugin组合Agent、工具资源及沙箱core，不依赖其他plugin。Agent同一轮发送InitToolTasks和InitInference，等待两者回复后继续启动；InitToolTasks只挂载空集合，不执行工具。

## 分组宿主接口

分组宿主API已实现，工具只使用新接口，不保留旧方法别名。主目录资源可同步使用；正在运行的旧daemon与旧数据库不会自动重启或迁移。

```text
margatroid
├── context（包含arguments）
├── log(level, message, details?)
├── wait_any(operations)
├── files：read、write、list、info、create_directory、matches
├── data：encode、decode、null
├── text：search
├── process：start
├── driver：mcl、wait
├── tools：register
└── task：return_result、finish、append_output、replace_output、read_output、send、receive
```

上下文只有一个入口margatroid.context；arguments收进其中，version不再向脚本公开，null归data。公共方法按操作对象分组；Driver协调、工具定义与执行任务分别使用driver、tools、task。子对象只读，错误身份调用明确失败，不持有另一方实际回调。configure只读元数据，执行确认前不开放IO或其他方法，不保留旧名称别名。

files:read返回完整原始字节，write仅写指定路径，不创建父目录。create_directory(path,recursive)显式负责目录创建。info合并原属性查询和规范路径解析，返回path、resolved_path、kind、size及modified_at_ns；path是查询项的绝对路径，kind描述其自身，resolved_path才是链接目标，失效链接可为空。list只列直接子项，返回name及同样信息，不递归或隐式过滤。所有操作服从当前执行环境权限；路径解析不代替沙箱，查询再读取也不承诺无竞态。

data:encode/decode(format,value)只接受json、yaml、toml、base64，不猜格式。结构化数据保留对象/数组、空容器和null，Base64处理字节。标准不支持的值、循环表或非法输入明确失败，不隐式删除或字符串化。

files:matches(path,pattern)仅匹配路径字符串，使用真正glob语义；text:search(pattern,content)提供通用Rust正则匹配及字节偏移，不替代工具的文件选择、逐行搜索或展示格式。修改时间字段使glob保留原排序；原regex、忽略文件、隐藏文件、二进制及include选择规则由Lua工具和随资源交付的公共脚本实现，以现有ripgrep行为为验收基线，不能用Lua pattern近似替代。

list-directory、glob、grep、write-file不再携带shell.lua/shell-client或调用rg、mkdir等外部进程。read-file、edit、read-image、skill-register也统一走files/data。只有提供shell和交互终端语义的bash、bash-exec继续使用shell-client；不新建Skill或文件工具bin。

```lua
local context = margatroid.context
local content = margatroid.files:read(context.arguments.path)
local value = margatroid.data:decode("yaml", content)
margatroid:log("info", "文件已解析", {path = context.arguments.path})
margatroid.task:return_result(value)
margatroid.task:finish(context.task_id)
```

日志和自动故障捕获保持上一版语义：主动日志及未捕获Lua异常写独立agent_logs，原始错误及traceback只入诊断记录，LLM获得分类后的失败。工具return后异常不重复回复，取消、timeout和finish控制退出不冒充Lua故障。Skill等预期业务失败直接返回说明，脚本自己pcall处理的异常需主动log，不用统一正则改写任意文本。

## 宿主预算配置

`config.toml`的`[tools]`直接提供三个容量字符串：`total_task_memory_limit`默认`"8G"`，限制当前daemon全部Agent的工具任务合计内存；`task_memory_limit`默认`"2G"`，限制单个worker、工具Lua VM及全部派生进程；`task_output_buffer_window`默认`"128M"`，是每任务在宿主侧的输出滚动窗口，不限制一次task_return结果。宿主中的Driver Lua VM和可信supervisor不计入工具内存额度。省略字段使用默认值，显式值须大于零，单任务上限可以高于总上限，实际仍同时受两级限制。修改配置后重启daemon生效，无需重新编译；当前没有热更新。示例见[配置说明](../config/README.md)。

宿主不设置脚本、参数、输入输出、诊断、消息队列、指令计数、解析深度或图片数量/大小配额。图片配额由工具作者决定，宿主仍校验有效的图片编码和结果格式。任务总内存由Linux cgroup v2强制执行，配置指定的委派根优先，否则使用systemd用户scope。任务启动前核对父、子两级memory.max和swap设置；全局父组不启用整组OOM终止，任务子组启用。总额度不足时内核可能终止一个或多个任务，不能假定只影响正在申请内存的任务。所有Agent的活跃工具任务共享一个父组，每任务单独清理子组，最后一个任务完成清理后释放父组。缺少支持时明确失败。宿主和负责回收的可信supervisor留在组外。容量单位K/M/G/T大小写不敏感，按1024进制解释。解析库本身的格式与实现限制仍适用，循环Lua table仍不能编码。

缓存超限自动滚出最旧内容，并调整cursor。超长字符串保留能放入窗口的UTF-8尾部，无法放入的结构化值整项滚出，不因窗口满返回失败。滚出不改变replaced标记；只有task_replace保留原替换语义。工具自行声明的timeout_ms及文件、结果策略不受本配置代替。

## 调用回复与任务结束

后端不定义长短任务类型。每次调用注册task_id并启动工具Lua；工具自行决定何时task_return，以及何时task_finish。task_return完成原tool_call的一次回复，不释放Lua、进程或输入通道。finish即清理：终止执行并回收真实资源，从ToolTasks移除任务和缓存，不保留已finish状态。

```lua
-- 立即完成的工具
local result = do_work()
margatroid.task:return_result(result)
margatroid.task:finish(context.task_id)

-- 后台工具
margatroid.task:return_result({ task_id = context.task_id })
for _, item in ipairs(items) do
    margatroid.task:append_output(process(item))
end
margatroid.task:finish(context.task_id)
```

工具请求在System受理时必须匹配当前非空turn；无turn直接返回中止工具结果而不创建任务。回合中止先清turn，再取消推理，并原子强制return所有尚未return的工具、触发终止清理；已return未finish后台任务保留。正常return与强制return竞争同一共享回复端，只交付一次。return不会结束任务，finish才清理并移除。

任务自然退出也完成清理并从集合移除；未task_return就退出，原调用返回MissingReturn。重复task_return报错，不产生第二个工具结果。已回复任务后续异常写入可查询error，不能重复完成原tool_call。

每个工具脚本必须定义configure(arguments, context)，返回本次显式配置；配置函数成功返回并经后端确认后，才调用execute(arguments, context)。不在工具清单或模型参数中强制添加timeout，限时由脚本作者决定，可根据本次arguments计算。缺失配置、缺少字段、未知字段、负数、非整数或无法表示的限时直接拒绝，后端不提供默认限时。

```lua
function configure(arguments, context)
    return { timeout_ms = 30000 } -- 0 表示不限存活时长
end

function execute(arguments, context)
    margatroid.task:return_result({ ok = true, content = "done" })
    margatroid.task:finish(context.task_id)
end
```

唯一配置timeout_ms由ToolTask保存，正整数为任务允许存活的毫秒数，0明确表示不限时。存活预算从配置确认后开始；task_return不解除预算，持久任务也按作者声明到期。配置阶段使用同一个Lua状态，只允许纯计算，仅可访问对象元数据，尚未开放IO、OS及对象的进程、任务、解析方法；不得在脚本顶层捕获这些能力。Lua内存上限覆盖顶层加载与配置阶段；取消hook保留，不再计数Lua指令。配置阶段没有默认超时，外部取消仍通过监督者终止worker。

shell、宿主请求和输入等待均服从同一ToolTask存活预算，不接受独立shell timeout_ms，不另设宿主或清理限时。到期请求取消，等待进程真实清理后才完成，不因到期丢弃监督。内置普通工具声明30秒，bash_exec声明120秒，持久bash声明0不限时；这些值写在各工具Lua配置函数中，后端没有对应默认常量。

## 缓存与输入

宿主创建任务时将共享句柄存入集合，克隆一份交给工作任务；工作任务还持有集合的Arc句柄，可查询同Agent的其他条目，不借用World。独立worker经既有双向宿主协议访问这些共享句柄，不跨进程传递Arc。

ToolTasks按任务id保存控制句柄、输出缓存、cursor、return状态及error，不混入推理或存储任务。缓存保留结构化值序列；task_read返回前将本次未读值中的相邻字符串直接拼接为一项，不添加分隔符，非字符串值原样保留并维持顺序。

| Lua函数 | 行为 |
| --- | --- |
| task_write(value) | 追加一个完整值，保留读取指针 |
| task_replace(value) | 整体替换为单个值，读取指针归零 |
| task_return(value) | 回复原调用一次，Lua继续运行 |
| task_finish(task_id) | 结束自身，或请求结束另一个任务并等待清理 |
| task_read(task_id) | 返回未读值和error，推进指针 |
| task_send(task_id, value) | 将值投递到目标Lua输入通道 |
| task_receive() | 当前Lua异步等待一条输入 |

task_read同时返回replaced标记：上次读取后发生覆盖为true，否则为false；读取后清除标记。读缓存及推进指针在同一次短锁内完成；覆盖也使用此锁。连续覆盖只保留最新值。缓存按窗口自动滚出旧内容；输入通道不附加数量配额。查询和控制仅限同一Agent，不根据任务id跨Agent访问。

发送成功表示已入队，不代表对方处理成功。输入接收方是Lua；转发进程、终端或网络的协议由具体工具定义，ToolTasks不解析输入语义。finish清理并移除后，read/send/finish均返回TaskMissing，没有完成缓存或completed字段。

## 任务控制工具

任务读取工具是普通即时工具，参数为task_id：调用task_read，将查询结果task_return，再task_finish自身。任务输入工具接受task_id和value，调用task_send并回复投递结果。关闭任务工具调用task_finish目标，清理成功后回复，再结束自身。

自身task_finish必须终止后续Lua语句，由外部监督任务回收VM，不能等待自己的JoinHandle。关闭其他任务成功即已清理并移除任务及缓存。清理失败及时返回错误、保留未finish监督责任，不宣称已finish。原调用return或读取游标推进不会删除活任务。

## bash与交互终端

执行阶段保留标准库路径信息，宿主并发等待使用margatroid:wait_any；os.execute、io.popen拒绝并提示使用margatroid.process:start。margatroid对象不提供shell、terminal_open。bash工具通过资源Lua调用process_open启动独立shell-client bin，shell IPC协议、PTY及VT100渲染由工具与bin负责，与MCP client使用相同的通用进程边界。

bash工具无调用参数，打开终端后返回task_id，收到的task_send消息由Lua转发，输出由Lua选择task_write或task_replace。普通终端滚屏保留滚走的行并继续追加，不把窗口移动当覆盖；光标改写及alternate screen仍使用替换语义。终端历史上限由工具传给独立bin，默认128MiB；宿主不附加原始管道配额；任务缓存按配置窗口自动滚出旧内容。进程、PTY、终端模拟器和追加或覆盖的判断属于工具自身设计，不作为Margatroid任务机制的接口要求。

## 执行与清理

工具执行按alias查询当前Tools完整记录，使用记录中的input_schema校验参数，按记录path读取main.lua程序，并按当前选择路径读取沙箱策略。描述与schema由Tools缓存，导入后文件metadata变化不会隐式改变记录；不重新加载清单或schema覆盖记录。

Lua在独立runner执行。可信supervisor在工具沙箱外，worker及其后代受策略限制；有选择而后端不可用时失败，不退化为无沙箱。双向版本化协议区分Return、Write、Replace和宿主请求，stdout只承载协议帧，诊断写stderr。

原调用回复后监督任务继续持有进程、连接及清理责任。取消不撤销已发生副作用，不直接abort监督任务。VM、进程、后代、管道和临时文件真实收口后即完成finish并移除任务；清理失败保留控制能力及error。Agent关闭时请求结束仍存活工具任务并等待；Agent重启时若有未完成工具任务直接拒绝，不代替调用方取消它们。

工具结果由MCL转换为消息；Tool不追加历史、不调用推理、不结束Agent回合。不新增长短任务分类，回合中止依据return状态决定是否终止。

LLM可见的工具失败保留原调用ID、工具名、failed标记、错误类别及说明，剥离Lua stack traceback；成功工具输出保持原样。task_read、task_send、task_finish资源直接调用通用任务能力，宿主错误经原执行监督回复，不在Lua里pcall后把整个错误堆栈拼成工具内容。TaskMissing仍是正常可处理的失败结果，不转换为成功，也不恢复已finish的任务。

## 动态入口与原始进程

工具注册接口目标为margatroid.tools:register；底层仍为Tools完整记录与原始子进程管道，本轮文档不表示运行中的daemon已经更新。

普通导入与动态注入都向Agent.Tools增加包含描述、schema、path的记录；推理每次遍历Tools，工具调用按名称查询。Tool执行侧从记录取得schema，按path读取程序。宿主提供通用文件、文本、数据与进程能力，MCP连接与协议由工具资源及同目录client bin负责；bin并发推进输入写入、stdout/stderr读取和带ID响应路由。

tool_inject由宿主固定当前资源路径，owner_task_id由注入参数显式选择，省略为静态工具，整批登记，任一定义无效或alias冲突整批失败，不增加System或专用事件。调用入口的owner_task_id、binding通过context传给同一脚本，模型参数不能改写它们；不额外增加工具清单或可见性步骤。

MCP注册任务A连接成功、注入入口后return但不finish；使用入口的任务B从私有context取得owner_task_id和binding，运行同一Lua脚本，把带request ID的调用发送给A，得到结果后return并finish。A连续转发带ID的请求并路由乱序响应。A清理时只移除Tools中属于它的记录，不等待或取消已受理的B；B按自身超时、取消与工具内部通信结果处理。B先结束时，A的迟到回复遇TaskMissing仅清除该请求关联。

process_open使用原始stdin/stdout/stderr管道，不经过PTY；write/read保持字节，工具自己处理消息边界、UTF-8分片和多条消息。输入写入、stdout与stderr读取需并行推进，避免双向背压互相阻塞。子进程和后代受ToolTask唯一存活预算、取消及finish清理约束。
