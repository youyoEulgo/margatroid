# MclPlugin

MCL 是 Model Context Language 的运行时。`base.lua` 是 Agent 的控制程序，
不是静态配置文件。它定义 Block、消息分流、pending tool、模型请求和工具调用
循环；Rust 不得再实现一套隐式的 User/Assistant/Tool 分支。

## 唯一控制流

```text
AgentImage/base.lua
    -> 每个 Agent 一个独立 Lua Driver 协程
    -> handle(command, binding)
    -> MCLPlugin 执行单条原子命令
    -> command response
    -> Lua 根据 response 继续执行
    -> Lua 产生 inference/tool_call/finish effect
```

AgentPlugin 只负责将外部 `AgentMessage` 放入 Driver mailbox、执行 `MclEffect`、
维护 Agent 状态和历史记录。AgentPlugin 不判断 Assistant 是否有 tool call，也不决定
何时进行下一次 inference。

## Base Driver

```lua
handle("IMPORT prompt:system/soul:latest AS soul")
handle("IMPORT skill:local/code-review:latest AS review")
handle("IMPORT tool:local/list-directory:latest AS list_dir")

handle([[ CREATE MESSAGE_BLOCK msg (
    system MESSAGE,
    conversation MESSAGE,
    pending_tool TOOL_CALL,
) ]])
handle([[ CREATE TOOL_BLOCK tool (
    tool_default TOOL,
    tool_dynamic TOOL,
) ]])
handle([[ CREATE REQUEST_BLOCK req (
    SELECT system, conversation FROM msg AS ctx,
    SELECT tool_dynamic FROM tool AS vis,
) ]])

handle("INJECT soul TO system FROM msg")
handle("INJECT review, list_dir TO tool_default FROM tool")
handle("INJECT SELECT tool_default FROM tool COVER tool_dynamic FROM tool")

while true do
    local message = handle("EMIT EFFECT start")
    if message.type == "user" then
        handle("INJECT ? TO conversation FROM msg", message)
        handle("EMIT EFFECT inference (req)")
    elseif message.type == "assistant" then
        handle("INJECT ? TO conversation FROM msg", message)
        if #(message.tool_calls or {}) > 0 then
            for _, call in ipairs(message.tool_calls) do
                handle("INJECT ? TO pending_tool FROM msg", call)
            end
            handle("EMIT EFFECT tool_call ?", message.tool_calls)
        else
            handle("EMIT EFFECT finish")
        end
    elseif message.type == "tool" then
        handle("INJECT ? TO conversation FROM msg", message)
        handle("DELETE pending_tool FROM msg WHERE id == ?", message.tool_call_id)
        if #handle("SELECT pending_tool FROM msg") == 0 then
            handle("EMIT EFFECT inference (req)")
        end
    end
end
```

Rust runtime必须执行这段程序，不得按文件名将其替换为生成的 handler，也不得从
`base.lua` 文件名推断消息规则。

## Command Boundary

每次 `handle` 是一个独立事务：

```text
parse -> resolve names/bindings -> clone AgentMcl -> validate -> commit -> emit effect
```

第一阶段标准接口：

```text
msg.system:        Vec<Message>
msg.conversation:  Vec<Message>
msg.pending_tool:  Vec<ToolCall>
tool.tool_default: Vec<ResourceMapEntry>
tool.tool_dynamic: Vec<ResourceMapEntry>
req.context:       msg.system + msg.conversation
req.tools:         tool.tool_dynamic
```

`pending_tool` 是唯一工具完成状态。Tool Message删除一个匹配的 ToolCall；只有数组为空
时 Driver 才能请求下一次 inference。

## Driver Mailbox

每个 Agent 只有一个有序 mailbox。`EMIT EFFECT start` 消费最早的消息；队列为空时 Lua
协程等待。User、Assistant 和 Tool 均通过同一个 mailbox，Rust 不做消息类型分流。

## Effects

```text
RequestInference { request_block }
ExecuteTools     { calls }
FinishTurn
```

Effect system只将它们转换为 inference、tool 和 Agent status 事件；不得修改 MCL Block，
也不得自行产生下一步 effect。

## Resource Imports

`IMPORT` 通过 ResourceMap 解析 alias。资源不可用时记录 Unavailable 并报告，但不终止
Agent 创建。Prompt资源注入 MESSAGE 字段时转换为 Message；可执行资源注入 TOOL 字段。

## 删除项

以下实现废除，生产代码不得继续引用：

```text
MclProgram handler/predicate/statement compiler
MclRuntimeEvent
MclToolExchange
execute_on_state
AgentPlugin::handle_agent_message tool loop
base.lua filename-based generated fallback
```

旧调用方必须迁移到 `MclDriverSource`、`MclRuntimeMessage` 和 Driver mailbox，禁止添加兼容
执行器让旧循环继续工作。
