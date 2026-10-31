# Tool

tool_core定义ToolTasks、工具请求、结果和错误，只依赖mecs及通用库。tool_plugin组合Agent、工具资源及沙箱core，不依赖其他plugin。Agent同一轮发送InitToolTasks和InitInference，等待两者回复后继续启动；InitToolTasks只挂载空集合，不执行工具。

## 调用回复与任务结束

后端不定义长短任务类型。每次调用注册task_id并启动工具Lua；工具自行决定何时task_return，以及何时task_finish。task_return完成原tool_call的一次回复，不释放Lua、进程或输入通道。task_finish结束执行，真实清理后设置completed，保留条目和输出。

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

任务不绑定turn，回合开始、结束、中止都不取消或修改工具任务。task_return及原reply释放不结束任务；只有工具结束、自身错误或显式任务控制影响任务生命周期。

任务自然退出也进入清理并记录完成；未task_return就退出，原调用返回MissingReturn。重复task_return报错，不产生第二个工具结果。已回复任务后续异常写入可查询error，不能重复完成原tool_call。

## 缓存与输入

宿主创建任务时将共享句柄存入集合，克隆一份交给工作任务；工作任务还持有集合的Arc句柄，可查询同Agent的其他条目，不借用World。独立worker经既有双向宿主协议访问这些共享句柄，不跨进程传递Arc。

ToolTasks按任务id保存控制句柄、输出缓存、cursor、completed及error，不混入推理或存储任务。缓存是结构化值序列，不是强制拼接的字符串。

| Lua函数 | 行为 |
| --- | --- |
| task_write(value) | 追加一个完整值，保留读取指针 |
| task_replace(value) | 整体替换为单个值，读取指针归零 |
| task_return(value) | 回复原调用一次，Lua继续运行 |
| task_finish(task_id) | 结束自身，或请求结束另一个任务并等待清理 |
| task_read(task_id) | 返回未读值、completed和error，推进指针 |
| task_send(task_id, value) | 将值投递到目标Lua输入通道 |
| task_receive() | 当前Lua异步等待一条输入 |

task_read同时返回replaced标记：上次读取后发生覆盖为true，否则为false；读取后清除标记。读缓存及推进指针在同一次短锁内完成；覆盖也使用此锁。连续覆盖只保留最新值。缓存及输入通道均有预算，超限明确返回错误，不静默丢失未读条目。查询和控制仅限同一Agent，不根据任务id跨Agent访问。

发送成功表示已入队，不代表对方处理成功。输入接收方是Lua；转发进程、终端或网络的协议由具体工具定义，ToolTasks不解析输入语义。任务已结束时拒绝新输入及缓存写入，仍允许读取剩余输出。

## 任务控制工具

任务读取工具是普通即时工具，参数为task_id：调用task_read，将查询结果task_return，再task_finish自身。任务输入工具接受task_id和value，调用task_send并回复投递结果。关闭任务工具调用task_finish目标，清理成功后回复，再结束自身。

自身task_finish必须终止后续Lua语句，由外部监督任务回收VM，不能等待自己的JoinHandle。关闭其他任务不删除记录或缓存；清理失败返回错误，不伪造completed。缓存条目不会因一次读取或原调用返回自动删除。

## bash与交互终端

bash工具无调用参数，打开终端后返回task_id，收到的task_send消息由Lua转发，输出由Lua选择task_write或task_replace。进程、PTY、终端模拟器和追加或覆盖的判断属于工具自身设计，不作为Margatroid任务机制的接口要求。

## 执行与清理

工具执行前按alias重新读取清单、schema和程序，按当前选择路径读取沙箱策略；实际参数按本次schema校验。材料仅用于本次运行，不恢复ResourceID快照或长期正文缓存。

Lua在独立runner执行。可信supervisor在工具沙箱外，worker及其后代受策略限制；有选择而后端不可用时失败，不退化为无沙箱。双向版本化协议区分Return、Write、Replace和宿主请求，stdout只承载协议帧，诊断写有界stderr。

原调用回复后监督任务继续持有进程、连接及清理责任。取消不撤销已发生副作用，不直接abort监督任务。VM、进程、后代、管道和临时文件真实收口后才标记completed；清理失败保留控制能力及error。Agent关闭时请求结束仍存活工具任务并等待；Agent重启时若有未完成工具任务直接拒绝，不代替调用方取消它们。

工具结果由MCL转换为消息；Tool不追加历史、不调用推理、不结束Agent回合。工具任务与turn无关，不新增长短任务分类。
