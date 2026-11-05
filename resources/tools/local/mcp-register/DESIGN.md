# MCP Lua 工具设计

本文是已确认、待实现的工具资源设计，只修改文档，不代表宿主已有tool_inject或process_open能力。Margatroid不实现或识别MCP协议；协议只存在于本工具Lua与同目录独立编译的mcp-client二进制。它不是新的Margatroid领域或专用服务插件。

# 资源布局

```text
mcp-register/latest/
├── tool.toml--正常MCP注册工具的名称和描述
├── input.schema.json--注册MCP server的参数schema
├── main.lua--同一脚本中的注册分支与使用分支
└── bin/
    └── mcp-client--独立交付的编译产物
```

宿主不安装依赖、编译或下载二进制。资源清单声明工具已有依赖及运行需求；执行依旧受所选沙箱限制，网络/可执行文件不可用时返回明确工具错误，不自动绕过沙箱。

# 工具入口

```text
注册入口：mcp_register
    alias/description/schema为正常MCP注册工具；path指向本资源。
    owner_task_id=None，binding=None；模型只看到注册schema，不需要填写内部route字段。

动态入口：例如mcp_a、mcp_b、mcp_c
    alias/description/schema分别来自注册发现及Lua映射，每个入口按自己的schema校验。
    path同样指向mcp_register资源；没有复制Lua程序，也不创建专门的MCP执行器。
    owner_task_id由宿主固定为注入它们的注册任务A；binding是Lua生成的不透明数据，例如{remote_tool="原始工具名"}。
    模型只看到各入口自身定义；owner_task_id/binding不是模型arguments，不接受调用者改写。
```

分支依据宿主注入的context.owner_task_id是否存在，而非根据arguments是否恰好匹配注册schema猜测；远端工具可能有相同字段名，不能据此误走注册分支。每次调用都先configure，再execute。

# Lua入口

```lua
function configure(arguments, context)
    if context.owner_task_id == nil then
        return { timeout_ms = 0 } -- 注册成功后常驻
    end
    return { timeout_ms = 30000 } -- 使用分支由作者声明，可根据参数计算
end

function execute(arguments, context)
    if context.owner_task_id == nil then
        register_server(arguments, context)
    else
        call_remote(arguments, context)
    end
end
```

这里只给结构草图，不在本轮创建可运行main.lua或声称已完成MCP实现。工具configure中没有IO/进程宿主能力，不能在那里连接server；连接发生在execute注册分支。

# 注册分支：任务A

```text
1. 调用process_open，使用context.resource_dir定位bin，argv数组传参，打开原始stdin/stdout/stderr管道，不使用terminal/PTY。
2. Lua向bin发送注册/连接请求；bin完成MCP初始化、连接与tools/list，将发现的定义返回Lua。
3. Lua完整累积帧后解析，构造带alias、description、input_schema、binding的定义列表。
4. 调用tool_inject一次原子注入整批定义；宿主自动绑定当前资源path、A的task_id及Agent/VM代际。
5. 注入成功后task_return注册成功结果，不调用task_finish；A已return未finish，继续保留VM、进程与会话。
6. 事件循环同时消费task_receive、新的stdout响应、stderr诊断及进程退出，不阻塞在某个远端请求上等待。
```

连接或发现失败时不注入。注入冲突整批失败，关闭本次子进程并结束任务；已注入但尚未return时被中止，按未return任务规则终止，清理本A的入口，不留下半注册状态。A注册成功return不代表会话永远健康，之后连接断开必须沿错误和清理规则处理。

# 使用分支：任务B

```text
1. 宿主按动态入口自己的schema校验模型arguments，建立B对A的通用所有者依赖。
2. 新任务B运行同一个main.lua，context包含本次alias、owner_task_id=A及入口binding。
3. task_send(A, request)，带本次唯一请求ID、自身任务ID、远端工具名及arguments。
4. B通过task_receive等待给自己的反馈，同时受自身timeout_ms/中止/所有者失效约束。
5. 校验响应ID，将远端结果转换为普通tool结果；task_return后立即task_finish(context.task_id)。
```

B在取得实际远端结果前不return“已提交”，否则它会被当作已return的后台任务而不再由本轮中止清理。task_return与task_finish之间不启动新长工作。

# 工具内部消息

```lua
-- B -> A，通过已有task_send
{
    operation = "call",
    id = context.task_id,
    reply_to = context.task_id,
    tool = context.binding.remote_tool,
    arguments = arguments,
}

-- A -> B，通过已有task_send
{
    id = request.id,
    result = { ok = true, content = "远端结果" },
}
```

这些字段属于工具内部协议，不成为Margatroid网络协议、工具schema公共参数或宿主MCP类型。task_send成功仅表示入队，远端是否成功以匹配响应为准。响应通过B的输入通道交付，不使用task_read共享游标取结果。

# Lua与bin的通信

```text
A -> bin stdin：一行一个JSON请求，包含唯一id及operation。
bin -> A stdout：一行一个JSON响应，包含相同id，可与其他响应乱序。
bin -> A stderr：独立诊断，不混入stdout；A必须持续消费或明确舍弃。
```

JSON Lines是本工具选择，process宿主仅传字节。Lua在缓冲区收齐换行前不能解析，支持一块含多帧、一帧拆多块、UTF-8跨读取切分。单帧/累计缓冲/在途请求数量必须有工具自身预算；超限明确失败，不能截断后伪造合法响应。实际MCP server的传输、版本协商及错误处理全部在bin内完成，stdin/stdout协议并不要求与外部MCP wire相同。

# 并发

```text
B1 ─┐
B2 ─┼─task_send→ A的单一任务输入循环 ─write完整请求→ mcp-client
B3 ─┘                                            ├─并发请求1
                                                  ├─并发请求2
                                                  └─并发请求3
B2 ←task_send─ A按pending[id]路由 ←stdout响应2
B1 ←task_send─ A按pending[id]路由 ←stdout响应1
B3 ←task_send─ A按pending[id]路由 ←stdout响应3
```

A维护pending[id]=reply_to。先登记关联再发送，响应只能消费一次，未知或重复ID明确丢弃/报协议诊断，不转发给其它B。发送一个请求后立即回事件循环接收其他任务请求和子进程输出，不能等此请求完成再受理下一条。bin必须支持多个在途MCP请求，按id返回响应；独占管道读取只用于保证字节和路由正确，不作为请求串行化理由。

管道背压也不能卡住stdout读取：A使用单独的有界写队列及唯一写循环，接收/响应读取与write并行推进；写队列满时明确拒绝该请求，不在事件循环里无限等待写入，同时放任bin的stdout堵满。stderr同样独立消费。该并发由Lua异步协程和已有await_any等执行能力组织，不要求宿主识别MCP请求。

普通MCL工具批次仍先预发所有ToolExecuteRequest、各带oneshot，再并发收齐并按原顺序返回；动态入口遵循同一执行机制。每个B独立ToolTask、reply和输入邮箱，不相互消费结果。

# 错误、取消与结束

```text
B出错：每次可关联调用返回自身原call_id/alias的failed结果，finish清理B；不使A或其它B退出。
B超时/被中止：未return时宿主原子强制return并清理B；A迟到task_send遇TaskMissing，只丢该关联。
远端取消：工具可以定义带id的cancel消息，由A/bin处理；Margatroid不解析MCP取消协议，不保证本地中止撤销已发生远端副作用。
A的bin断开：A将全部pending请求逐一回复失败，停止新受理并finish；进程EOF、退出及错误均需明确处理，不无限task_receive。
A失联/finish：任务监督删除Tools中owner_task_id=A的记录，并查同一ToolTasks里标记owner_task_id=A的等待任务，逐个返回明确工具错误并清理；不另设依赖管理器，也不依赖A还能发消息才能释放B。
A真正finish：子进程及后代、管道、VM和临时资源清理完成，ToolTasks移除A，所属动态定义不再可见；清理失败保留监督，不保存已finish条目。
Agent/Driver重启：动态定义与任务均不持久化、不恢复旧标记；既有未finish任务仍按当前重启限制处理，关闭按原流程真实清理。
```

# Agent工具集合

```text
IMPORT TOOL只向Agent的Tools增加记录；tool_inject也向同一个Tools增加记录。
每条记录都有alias、description、input_schema、path；动态记录额外标记owner_task_id和binding。
每次推理直接遍历Tools重新构造schema，不带额外工具列表、不读取配置、不依赖执行脚本。
工具调用拿请求名称查当前Tools，找到就按该记录执行，找不到就返回工具错误。A清理时移除所属记录。
```

# 持有关系

```text
Agent
├── Tools统一集合
│   ├── mcp_register：普通记录
│   └── mcp_a/mcp_b/mcp_c：动态记录，path相同，owner=A，binding不同
└── ToolTasks
    ├── A：已return未finish，注册分支
    │   ├── Lua事件循环、pending关联表
    │   └── ProcessSession -> mcp-client -> MCP server长连接
    └── B1/B2/B3：未return，使用分支
        └── 各自请求/响应及通用owner依赖，结果到达后return+finish
```
