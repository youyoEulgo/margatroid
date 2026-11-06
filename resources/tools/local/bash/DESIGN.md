# 伪代码格式

```text
板块使用一级标题；方法使用以下格式，逻辑写入行为块。
method(arguments) -> ReturnType
    行为：
        执行逻辑
-- 仅作简短补充说明
持有关系使用树形图。
```

# 板块定义

```text
# lib     latest/main.lua   configure与execute
# handler latest/shell.lua  工具私有进程协议
# types   latest/shell.lua  管道缓冲及帧队列
```

# lib

```text
configure(arguments, context) -> ToolConfiguration
    行为：
        return {timeout_ms=0}
execute(arguments, context) -> ()
    行为：
        terminal = client.terminal({max_output_bytes=128MiB})
        await ready; task_return({task_id=context.task_id})
        loop await_any(terminal.read, task_receive)
            output.replace: task_replace(text)
            output.text非空: task_write(text)
            输入: require text:string, enter?:bool; terminal.write(text .. 可选CR)
            无效输入: task_write({error=参数错误}); continue
            exit: task_write({exit_code}); break
        task_finish(context.task_id)
```

# handler

```text
client = dofile(context.resource_dir .. "/shell.lua")(context)
client.exec(command, options) -> result
    行为：
        require options.timeout_ms == nil
        process_open(resource_dir .. "/bin/shell-client", arguments=[])
        send {operation="exec", command, max_output_bytes, pty}
        consume result中的stdout、stderr、code、exit_code、signal、truncated、terminal
        drain stdout及stderr至EOF; await process.wait

client.terminal(options) -> terminal
    行为：
        process_open(resource_dir .. "/bin/shell-client", arguments=[])
        send {operation="terminal", max_output_bytes}
        await ready

terminal.read() -> {text, replace} | {eof, exit_code}
    行为：
        output: 返回{text, replace}
        exit: 保存final_exit；排空双pipe并wait，返回{eof=true, exit_code}
        drain期间被取消：下次读取继续使用final_exit

terminal.write(text) -> ()
    行为：
        queue {operation="write", text}

terminal.close() -> ()
    行为：
        queue close
        flush队列；排空双pipe；wait

pump(writing) -> bool
    行为：
        await_any(stdout读取, stderr读取, 允许时queued_write)
        stdout -> 累积完整JSONL帧 -> frames
        writer: 重试同一完整frame; 完成才出队
        error帧: 传播原message
        返回是否仍有可推进的管道操作
```

# types

```text
state:
    buffer, frames, writes
    stdout_open, stderr_open, finished
    status, close_requested
terminal:
    final_exit
frame_limit = max_output_bytes * 12 + 8192 -- 两条流各有预算及JSON转义
```

# 持有关系

```text
ToolTask
└── 工具Lua及shell.lua
    └── ProcessSession -- 通用宿主清理
        └── 独立shell-client
            └── shell进程及可选PTY
```
