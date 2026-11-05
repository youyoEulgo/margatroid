# Tool

tool_core定义ToolTasks、工具请求、结果和错误，只依赖mecs及通用库。tool_plugin组合Agent、工具资源及沙箱core，不依赖其他plugin。Agent同一轮发送InitToolTasks和InitInference，等待两者回复后继续启动；InitToolTasks只挂载空集合，不执行工具。

## 调用回复与任务结束

后端不定义长短任务类型。每次调用注册task_id并启动工具Lua；工具自行决定何时task_return，以及何时task_finish。task_return完成原tool_call的一次回复，不释放Lua、进程或输入通道。finish即清理：终止执行并回收真实资源，从ToolTasks移除任务和缓存，不保留已finish状态。

```lua
-- 立即完成的工具
local result = do_work()
task_return(result)
task_finish(context.task_id)

-- 后台工具
task_return({ task_id = context.task_id })
for _, item in ipairs(items) do
    task_write(process(item))
end
task_finish(context.task_id)
```

工具请求在System受理时必须匹配当前非空turn；无turn直接返回中止工具结果而不创建任务。回合中止先清turn，再取消推理，并原子强制return所有尚未return的工具、触发终止清理；已return未finish后台任务保留。正常return与强制return竞争同一共享回复端，只交付一次。return不会结束任务，finish才清理并移除。

任务自然退出也完成清理并从集合移除；未task_return就退出，原调用返回MissingReturn。重复task_return报错，不产生第二个工具结果。已回复任务后续异常写入可查询error，不能重复完成原tool_call。

每个工具脚本必须定义configure(arguments, context)，返回本次显式配置；配置函数成功返回并经后端确认后，才调用execute(arguments, context)。不在工具清单或模型参数中强制添加timeout，限时由脚本作者决定，可根据本次arguments计算。缺失配置、缺少字段、未知字段、负数、非整数或无法表示的限时直接拒绝，后端不提供默认限时。

```lua
function configure(arguments, context)
    return { timeout_ms = 30000 } -- 0 表示不限存活时长
end

function execute(arguments, context)
    task_return({ ok = true, content = "done" })
    task_finish(context.task_id)
end
```

唯一配置timeout_ms由ToolTask保存，正整数为任务允许存活的毫秒数，0明确表示不限时。存活预算从配置确认后开始；task_return不解除预算，持久任务也按作者声明到期。配置阶段使用同一个Lua状态，只允许纯计算，尚未安装IO、OS、shell、terminal、task、json等宿主能力；不得在脚本顶层捕获这些能力。内存和指令预算覆盖顶层加载与配置阶段，防止配置本身无限执行；这些是资源预算，不是后端默认时间限制。

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

task_read同时返回replaced标记：上次读取后发生覆盖为true，否则为false；读取后清除标记。读缓存及推进指针在同一次短锁内完成；覆盖也使用此锁。连续覆盖只保留最新值。缓存及输入通道均有预算，超限明确返回错误，不静默丢失未读条目。查询和控制仅限同一Agent，不根据任务id跨Agent访问。

发送成功表示已入队，不代表对方处理成功。输入接收方是Lua；转发进程、终端或网络的协议由具体工具定义，ToolTasks不解析输入语义。finish清理并移除后，read/send/finish均返回TaskMissing，没有完成缓存或completed字段。

## 任务控制工具

任务读取工具是普通即时工具，参数为task_id：调用task_read，将查询结果task_return，再task_finish自身。任务输入工具接受task_id和value，调用task_send并回复投递结果。关闭任务工具调用task_finish目标，清理成功后回复，再结束自身。

自身task_finish必须终止后续Lua语句，由外部监督任务回收VM，不能等待自己的JoinHandle。关闭其他任务成功即已清理并移除任务及缓存。清理失败及时返回错误、保留未finish监督责任，不宣称已finish。原调用return或读取游标推进不会删除活任务。

## bash与交互终端

bash工具无调用参数，打开终端后返回task_id，收到的task_send消息由Lua转发，输出由Lua选择task_write或task_replace。普通终端滚屏保留滚走的行并继续追加，不把窗口移动当覆盖；光标改写及alternate screen仍使用替换语义。终端历史受output_bytes与cache_bytes中的较小预算限制，超出明确报OutputTooLarge，不静默丢弃旧行。进程、PTY、终端模拟器和追加或覆盖的判断属于工具自身设计，不作为Margatroid任务机制的接口要求。

## 执行与清理

工具执行前按alias重新读取清单、schema和程序，按当前选择路径读取沙箱策略；实际参数按本次schema校验。材料仅用于本次运行，不恢复ResourceID快照或长期正文缓存。

Lua在独立runner执行。可信supervisor在工具沙箱外，worker及其后代受策略限制；有选择而后端不可用时失败，不退化为无沙箱。双向版本化协议区分Return、Write、Replace和宿主请求，stdout只承载协议帧，诊断写有界stderr。

原调用回复后监督任务继续持有进程、连接及清理责任。取消不撤销已发生副作用，不直接abort监督任务。VM、进程、后代、管道和临时文件真实收口后即完成finish并移除任务；清理失败保留控制能力及error。Agent关闭时请求结束仍存活工具任务并等待；Agent重启时若有未完成工具任务直接拒绝，不代替调用方取消它们。

工具结果由MCL转换为消息；Tool不追加历史、不调用推理、不结束Agent回合。不新增长短任务分类，回合中止依据return状态决定是否终止。
