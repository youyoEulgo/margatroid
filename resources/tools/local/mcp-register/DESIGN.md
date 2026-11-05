# 伪代码格式

```text
板块使用一级标题；字段、函数及直接逻辑置于text伪代码块。
说明使用简短--注释；持有关系使用树形图。
```

# 板块定义

```text
工具入口       := 注册记录、动态记录与宿主上下文
Lua入口        := configure / execute
注册分支A      := register_server；注册return后保持pending循环
使用分支B      := call_remote；取得结果后return并finish
工具内部协议   := B <-> A消息、A <-> mcp-client字节管道
mcp-client     := MCP连接与多个在途请求的IO协议
结束与清理     := 失败、取消、owner失效及资源释放
Agent工具集合  := Tools直接构造schema与按名称查找
持有关系       := Agent / Tools / ToolTasks / 进程会话
```

# 工具入口

```text
mcp_register = {
    alias, description, input_schema = 注册工具定义,
    path = 本资源,
    owner_task_id = None,
    binding = None
}
mcp_a / mcp_b / mcp_c = {
    alias, description, input_schema = 发现结果及Lua映射,
    path = 同一本资源,
    owner_task_id = 宿主固定的注册任务A.task_id,
    binding = Lua生成的不透明数据，例如{remote_tool = 原始工具名}
}
模型arguments按当前入口input_schema校验
context携带alias、task_id、resource_dir、owner_task_id、binding
owner_task_id / binding不接受模型arguments改写
每次调用：configure(arguments, context) -> execute(arguments, context)
```

# Lua入口

```text
function configure(arguments, context)
    if context.owner_task_id == nil then
        return {timeout_ms = 0}       -- A注册成功后常驻
    end
    return {timeout_ms = 30000}       -- B由作者声明，可按参数计算
end

function execute(arguments, context)
    if context.owner_task_id == nil then
        register_server(arguments, context)
    else
        call_remote(arguments, context)
    end
end
-- configure无IO；连接在execute注册分支完成
```

# 注册分支A

```text
function register_server(arguments, context)
    process_open：context.resource_dir下bin/mcp-client，argv数组
    打开原始stdin / stdout / stderr管道，无terminal / PTY
    以并发协程持续读取stdout、消费或明确舍弃stderr、观察进程退出
    向bin发送带唯一id及operation的注册/连接JSON Line
    累积完整响应帧，取得初始化、连接及tools/list结果
    if 连接或发现失败 then
        不注入；关闭本次子进程；返回失败并finish
    end
    definitions = 发现结果映射出的{alias, description, input_schema, binding}列表
    tool_inject(definitions)          -- 一次原子注入整批
    -- 宿主固定path、owner_task_id=A、Agent/VM代际
    if 注入冲突 then
        整批失败；关闭本次子进程；返回失败并finish
    end
    task_return(注册成功结果)          -- 不task_finish；保留VM、进程与会话

    pending = {}                      -- pending[id] = reply_to
    有界写队列；唯一写协程按队列向stdin写入完整帧
    -- 写协程、stdout读取、stderr消费、task_receive并发推进
    while A未finish do
        await_any安排现有并发协程取得可处理项
        if 收到task_receive中的B请求 then
            if 写队列、在途数量或帧预算超限 then
                task_send(request.reply_to, {id=request.id, result=明确失败})
            else
                先登记pending[request.id] = request.reply_to
                将完整请求帧加入有界写队列
                -- 立即继续循环，不等待该请求完成
            end
        else if 收到stdout字节 then
            累积缓冲；仅对完整JSON Line解析；保留未完成尾帧
            for 每个完整响应 do
                if pending[response.id]存在 then
                    reply_to = pending[response.id]
                    删除pending[response.id]       -- 只消费一次
                    task_send(reply_to, {id=response.id, result=response.result})
                    if TaskMissing then 丢弃此关联 end
                else
                    丢弃未知/重复id，记录协议诊断
                end
            end
        else if 收到stderr字节 then
            消费诊断或明确舍弃
        else if EOF、进程退出、连接断开、IO/协议错误或缓冲预算超限 then
            停止受理新请求
            逐一向pending对应B回复明确失败并移除关联
            finish A，执行结束与清理
        end
    end
end
```

# 使用分支B

```text
-- 宿主先校验动态入口schema，建立B对A的通用owner依赖
function call_remote(arguments, context)
    request = {
        operation = "call",
        id = context.task_id,
        reply_to = context.task_id,
        tool = context.binding.remote_tool,
        arguments = arguments
    }
    task_send(context.owner_task_id, request)
    通过task_receive等待本B的反馈
    -- 等待受timeout_ms、中止与owner失效约束
    校验响应id，将result转换为普通tool结果
    task_return(实际结果或本次调用的明确失败)
    task_finish(context.task_id)       -- 中间不启动长工作
end
```

# 工具内部协议

```text
B -> A，task_send：
    {operation="call", id=B.task_id, reply_to=B.task_id,
     tool=context.binding.remote_tool, arguments=arguments}
A -> B，task_send：
    {id=request.id, result={ok=true, content="远端结果"}}
task_send成功 := 已入队；调用完成 := 收到匹配id的实际反馈
B响应经自身输入通道交付，不用task_read共享游标

A -> bin stdin  := 每行一个JSON请求，包含唯一id、operation
bin -> A stdout := 每行一个JSON响应，包含原id，允许乱序
bin -> A stderr := 独立诊断
原始字节 -> 累积缓冲 -> 完整换行帧 -> JSON解析
-- 支持多帧同块、单帧跨块、UTF-8跨块
单帧 / 累计缓冲 / 在途数 / 写队列均有工具预算
超限明确失败；不截断后伪造合法响应
```

# mcp-client

```text
注册请求 -> MCP初始化、连接、tools/list -> 定义响应
多个call请求 -> 多个在途MCP请求 -> 按原id输出结果，允许乱序
stdout只含协议响应；stderr独立输出诊断
外部MCP传输、版本协商、错误处理在bin内完成
-- Lua侧并发使用已有协程及await_any，不增加宿主接口
```

# 结束与清理

```text
B调用失败：
    返回本次原call_id / alias的failed结果；finish B
    A与其他B继续
B超时或中止且未return：
    宿主原子强制return并清理B；A继续
    A迟到task_send得到TaskMissing时只丢对应关联
远端取消：
    工具可定义带id的cancel消息，由A/bin处理
    -- 本地中止不保证撤销远端副作用
A在注入后、注册return前被中止：
    按未return任务规则终止；清理A所属入口
A失联或finish：
    任务监督移除Tools中owner_task_id=A的记录
    直接遍历同一ToolTasks中owner_task_id=A的等待B
    逐个返回明确工具错误并清理，不依赖A发消息
A真正finish：
    完成子进程及后代、管道、VM、临时资源清理
    ToolTasks移除A；动态定义不再可见
    清理失败则保留监督，不保存已finish条目
Agent / Driver重启：
    动态定义与任务不持久化，不恢复旧标记
    未finish任务遵循当前重启限制；关闭沿原流程真实清理
```

# Agent工具集合

```text
IMPORT TOOL -> Agent.Tools增加普通记录
tool_inject -> 同一Agent.Tools原子增加动态记录
每次推理：直接遍历Tools -> 构造schema
每次调用：按请求名称查当前Tools -> 按记录执行
未找到记录 -> 明确工具错误
-- 不增加工具列表、不读配置、不依赖执行脚本构造schema
普通MCL批次：预发全部ToolExecuteRequest及各自oneshot
              -> 并发收齐 -> 按原顺序返回
动态入口沿用该执行机制；每个B独立ToolTask、reply、输入邮箱
```

# 持有关系

```text
Agent
├── Tools
│   ├── mcp_register：普通记录
│   └── mcp_a / mcp_b / mcp_c：相同path，owner=A，各自binding
└── ToolTasks
    ├── A：注册分支，已return未finish
    │   ├── Lua并发循环、pending、有界写队列
    │   └── ProcessSession -> mcp-client -> MCP server长连接
    └── B1 / B2 / B3：使用分支，等待时未return
        └── 各自请求/响应、通用owner依赖；结果到达后return + finish
```
