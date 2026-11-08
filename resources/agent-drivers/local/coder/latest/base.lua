local function command(text, arguments)
    local result
    if arguments == nil then
        result = mcl(agent_info.entity, text)
    else
        result = mcl(agent_info.entity, text, arguments)
    end
    if type(result) == "table" and result.Error then
        local failure = result.Error
        failure.command = text
        error(failure, 0)
    end
    return result
end

local function settings()
    return command("CONFIG GET").Config
end

local function inference()
    return command("EFFECT inference (msg.instructions, session.conversation)").Inference.message
end

local function tool_call()
    return command("EFFECT tool_call (session.conversation[-1])").Array.Message
end

local function maybe_compact_context(message)
    local config = settings()
    local threshold = agent_info.model.context_window_tokens * config.compaction.threshold
    if not message.usage
        or message.usage.input_tokens + message.usage.output_tokens < threshold then
        return
    end
    local conversation = command("GET session.conversation").Array.Message
    local count = 0
    local split = 1
    for index = #conversation, 1, -1 do
        local kind = conversation[index].type
        if kind == "user" or kind == "assistant" then
            count = count + 1
            if count == config.compaction.keep_messages then
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
    local result = command("EFFECT inference (msg.instructions, msg.compact_context, msg.compact_prompt)")
    local summary = result.Inference
    command("EFFECT history_append ?", {{summary.message}})
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

local initialized, initialization_error = pcall(function()
    command("IMPORT TOOL tool:local/skill-register:latest AS skill_register")
    command("IMPORT MESSAGE prompt:local/coder-soul:latest AS soul")
    command("IMPORT MESSAGE prompt:local/compact:latest AS compact")
    command("IMPORT TOOL tool:local/list-directory:latest AS list_dir")
    command("IMPORT TOOL tool:local/read-file:latest AS read_file")
    command("IMPORT TOOL tool:local/read-image:latest AS read_image")
    command("IMPORT TOOL tool:local/write-file:latest AS write_file")
    command("IMPORT TOOL tool:local/edit:latest AS edit")
    command("IMPORT TOOL tool:local/grep:latest AS grep")
    command("IMPORT TOOL tool:local/glob:latest AS glob")
    command("IMPORT TOOL tool:local/bash:latest AS bash")
    command("IMPORT TOOL tool:local/bash-exec:latest AS bash_exec")
    command("IMPORT TOOL tool:local/task-finish:latest AS task_finish")
    command("IMPORT TOOL tool:local/task-send:latest AS task_send")
    command("IMPORT TOOL tool:local/task-read:latest AS task_read")
    command("IMPORT SANDBOX sandbox:local/workspace-write:latest AS workspace_write")
    command("IMPORT SANDBOX sandbox:local/read-only:latest AS read_only")

    command("CREATE TEMP BLOCK msg (instructions, compact_prompt, compact_context)")
    command("CREATE TEMP BLOCK entry (pending)")
    command("CREATE BLOCK session (conversation)")
    command("INJECT soul TO msg.instructions")
    command("INJECT compact TO msg.compact_prompt")
    command("EFFECT sandbox_use ?", {settings().sandbox})
    command("EFFECT ready")
end)
if not initialized then
    if type(initialization_error) == "table" and initialization_error.type == "error" then
        error(initialization_error.message, 0)
    end
    error(initialization_error, 0)
end

local function inject_context(message)
    for _, item in ipairs(message.messages or {}) do
        if item.type == "inject" then
            inject_context(item)
        else
            command("INJECT ? TO session.conversation[-1]", {{item}})
        end
    end
end

local pending_tool_results = 0
local cleanup_pending = false

local function process_next()
    -- A prior finish may have replied before background cleanup completed.
    -- Retry only after a wake, before accepting another turn.
    if cleanup_pending then
        command("EFFECT finish_turn")
        cleanup_pending = false
    end
    local message = command("GET entry.pending[0]").Array.Message[1]
    if message then
        command("INJECT [] TO entry.pending[0,1]")
        command("EFFECT history_append ?", {{message}})

        if message.type == "inject" then
            inject_context(message)
        elseif message.type == "user" then
            command("INJECT ? TO session.conversation[-1]", {{message}})
            command("EFFECT start_turn")
            local result = inference()
            command("INJECT ? TO entry.pending[-1]", {{result}})
        elseif message.type == "assistant" then
            if (message.content and message.content ~= "")
                or (message.reasoning and message.reasoning ~= "")
                or #(message.tool_calls or {}) > 0 or message.replay then
                command("INJECT ? TO session.conversation[-1]", {{message}})
            end
            if #(message.tool_calls or {}) == 0 then
                maybe_compact_context(message)
                command("EFFECT finish_turn")
            else
                local results = tool_call()
                pending_tool_results = #results
                command("INJECT ? TO entry.pending[-1]", {results})
                maybe_compact_context(message)
            end
        elseif message.type == "error" then
            pending_tool_results = 0
            command("EFFECT finish_turn")
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

-- Recovery commands are bounded: storage/finish failures must never recursively
-- report themselves or bypass the host's real turn cleanup state.
local function recover(failure)
    pending_tool_results = 0
    if failure.command ~= "EFFECT history_append ?" then
        pcall(command, "EFFECT history_append ?", {{
            {type = "error", message = failure.message, request_id = failure.request_id}
        }})
    end
    if failure.command == "EFFECT finish_turn" then
        cleanup_pending = true
    else
        local finished = pcall(command, "EFFECT finish_turn")
        cleanup_pending = not finished
    end
    wait()
end

while true do
    local ok, failure = pcall(process_next)
    if not ok then
        if type(failure) ~= "table" or failure.type ~= "error" then
            failure = {type = "error", message = tostring(failure)}
        end
        recover(failure)
    end
end
