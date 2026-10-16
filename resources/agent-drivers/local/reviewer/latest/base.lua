local COMPACTION_TOKENS = agent_info.model.context_window_tokens * 0.8
local COMPACTION_KEEP_MESSAGES = 16

local function command(text, arguments)
    if arguments == nil then
        return mcl(agent_info.resource_id, text)
    end
    return mcl(agent_info.resource_id, text, arguments)
end

local function inference()
    return command("EMIT EFFECT inference (msg.instructions, session.conversation) (session.visible)").Inference.message
end

local function tool_call()
    return command("EMIT EFFECT tool_call (session.conversation[-1]) (session.visible)").Array.Message
end

local function maybe_compact_context(message)
    if not message.usage
        or message.usage.input_tokens + message.usage.output_tokens < COMPACTION_TOKENS then
        return
    end
    local conversation = command("GET session.conversation").Array.Message
    local count = 0
    local split = 1
    for index = #conversation, 1, -1 do
        local kind = conversation[index].type
        if kind == "user" or kind == "assistant" then
            count = count + 1
            if count == COMPACTION_KEEP_MESSAGES then
                split = index
                break
            end
        end
    end
    if split == 1 then
        return
    end

    local earlier = {}
    local recent = {}
    for index, item in ipairs(conversation) do
        if index < split then
            earlier[#earlier + 1] = item
        else
            recent[#recent + 1] = item
        end
    end
    command("INJECT ? TO msg.compact_context", {earlier})
    local result = command("EMIT EFFECT inference (msg.instructions, msg.compact_context, msg.compact_prompt) ?", {{}})
    local summary = result.Inference
    command("EMIT EFFECT history_append ?", {{summary.message}})
    if summary.stop_reason == "completed" and summary.message.type == "assistant"
        and summary.message.content and summary.message.content ~= "" then
        local replacement = {
            {type = "user", content = "Conversation checkpoint:\n" .. summary.message.content}
        }
        for _, item in ipairs(recent) do
            replacement[#replacement + 1] = item
        end
        command("INJECT ? TO session.conversation", {replacement})
    end
end

command("IMPORT MESSAGE prompt:local/reviewer-soul:latest AS soul")
command("IMPORT MESSAGE prompt:local/compact:latest AS compact")
command("IMPORT TOOL tool:local/code-review:latest AS review")
command("IMPORT TOOL tool:local/list-directory:latest AS list_dir")
command("IMPORT TOOL tool:local/project-context:latest AS project_context")
command("IMPORT SANDBOX sandbox:local/workspace-write:latest AS workspace_write")
command("IMPORT SANDBOX sandbox:local/read-only:latest AS read_only")

command("CREATE TEMP BLOCK msg (instructions MESSAGE, compact_prompt MESSAGE, compact_context MESSAGE)")
command("CREATE TEMP BLOCK entry (pending MESSAGE)")
local session = command("CREATE BLOCK session (conversation MESSAGE, visible TOOL, sandbox_activated SANDBOX)").BlockOpened
command("INJECT soul TO msg.instructions")
command("INJECT compact TO msg.compact_prompt")
if session.created then
    command("INJECT review, project_context, list_dir TO session.visible")
    command("INJECT read_only TO session.sandbox_activated")
end
command("EMIT EFFECT sandbox_use (session.sandbox_activated)")

local function inject_context(message)
    for _, item in ipairs(message.messages or {}) do
        if item.type == "inject" then
            inject_context(item)
        elseif item.type ~= "assistant" or item.stop_reason ~= "cancelled" then
            command("INJECT ? TO session.conversation[-1]", {{item}})
        end
    end
end

local pending_tool_results = 0

command("EMIT EFFECT ready")

while true do
    local message = command("GET entry.pending[0]").Array.Message[1]
    if message then
        command("INJECT [] TO entry.pending[0,1]")
        command("EMIT EFFECT history_append ?", {{message}})

        if message.type == "inject" then
            inject_context(message)
        elseif message.type == "user" then
            command("INJECT ? TO session.conversation[-1]", {{message}})
            command("EMIT EFFECT start_turn")
            local result = inference()
            command("INJECT ? TO entry.pending[-1]", {{result}})
        elseif message.type == "assistant" then
            if message.stop_reason ~= "cancelled" then
                command("INJECT ? TO session.conversation[-1]", {{message}})
            end
            if #(message.tool_calls or {}) == 0 then
                if message.stop_reason ~= "cancelled" then
                    maybe_compact_context(message)
                end
                command("EMIT EFFECT finish_turn")
            else
                local results = tool_call()
                pending_tool_results = #results
                command("INJECT ? TO entry.pending[-1]", {results})
                maybe_compact_context(message)
            end
        elseif message.type == "tool_call" then
            command("INJECT ? TO session.conversation[-1]", {{message}})
            pending_tool_results = pending_tool_results - 1
            if pending_tool_results == 0 then
                local result = inference()
                command("INJECT ? TO entry.pending[-1]", {{result}})
            end
        end
    else
        wait()
    end
end
