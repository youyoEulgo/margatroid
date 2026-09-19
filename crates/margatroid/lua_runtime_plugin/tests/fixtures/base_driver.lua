-- Base Driver for every Agent image.
-- `mcl` is the host boundary injected by Margatroid.

local function mcl_command(command, binding)
    return mcl(agent_info.id, command, binding)
end

mcl_command("IMPORT prompt:system/soul:latest AS soul")
mcl_command("IMPORT prompt:user/compact:latest AS compact")
mcl_command("IMPORT tool:local/read-file:latest AS read_file")
mcl_command("IMPORT tool:local/write-file:latest AS write_file")
mcl_command("IMPORT tool:local/edit:latest AS edit")
mcl_command("IMPORT tool:local/grep:latest AS grep")
mcl_command("IMPORT tool:local/glob:latest AS glob")
mcl_command("IMPORT shell:local/bash:latest AS bash")
mcl_command("IMPORT tool:local/list-directory:latest AS list_directory")

mcl_command([[
    CREATE BLOCK msg (
        system_prompt MESSAGE,
        compact_prompt MESSAGE,
        compact_context MESSAGE,
        history_conversation MESSAGE,
        recent_conversation MESSAGE,
    )
]])

mcl_command([[
    CREATE BLOCK realtime_state (
        compact_context MESSAGE,
        history_conversation MESSAGE,
        recent_conversation MESSAGE,
    )
]])

mcl_command([[
    CREATE BLOCK tool (
        tool_default RESOURCE,
        tool_dynamic RESOURCE,
    )
]])

mcl_command([[
    CREATE REF_BLOCK req (
        REF_MERGE system_prompt, compact_context, history_conversation, recent_conversation FROM msg AS ctx,
        REF_MERGE tool_dynamic FROM tool AS vis,
    )
]])

mcl_command([[
    CREATE REF_BLOCK com (
        REF_MERGE system_prompt, compact_context, history_conversation, compact_prompt FROM msg AS ctx,
    )
]])

mcl_command("INJECT soul TO msg.system_prompt")
mcl_command("INJECT compact TO msg.compact_prompt")
mcl_command("INJECT read_file, write_file, edit, grep, glob, bash, list_directory TO tool.tool_default")
mcl_command("INJECT tool.tool_default TO tool.tool_dynamic")
mcl_command("LOAD STATE realtime INTO realtime_state")
mcl_command("INJECT realtime_state.compact_context TO msg.compact_context")
mcl_command("INJECT realtime_state.history_conversation TO msg.history_conversation")
mcl_command("INJECT realtime_state.recent_conversation TO msg.recent_conversation")

mcl_command("BIND realtime_state TO STATE realtime")

local function sync_realtime()
    mcl_command("INJECT msg.compact_context TO realtime_state.compact_context")
    mcl_command("INJECT msg.history_conversation TO realtime_state.history_conversation")
    mcl_command("INJECT msg.recent_conversation TO realtime_state.recent_conversation")
end

local function append_recent(message)
    mcl_command("INJECT ? TO msg.recent_conversation", message)
    sync_realtime()
    mcl_command("EMIT EFFECT history_append FROM ?", message)
end

local function move_old_recent_messages()
    local assistant_seen = false
    while true do
        local recent = mcl_command("GET msg.recent_conversation")
        if #recent == 0 then
            return
        end
        local first = recent[1]
        if first.type == "assistant" then
            if assistant_seen then
                return
            end
            assistant_seen = true
        end
        mcl_command("INJECT ? TO msg.history_conversation", first)
        mcl_command("INJECT [] TO msg.recent_conversation[0,1]")
    end
end

move_old_recent_messages()
mcl_command("EMIT EFFECT visibility_source FROM tool.tool_dynamic")
mcl_command("EMIT EFFECT default_visibility_source FROM tool.tool_default")
mcl_expose("tools", {
    visible = "tool.tool_dynamic",
    default = "tool.tool_default",
})

local MAX_CONTEXT_TOKENS = agent_info.model.context_window_tokens
local RECENT_CONTEXT_RATIO = 0.16
local COMPACTION_CONTEXT_RATIO = 0.80
local RECENT_CONTEXT_LIMIT = MAX_CONTEXT_TOKENS * RECENT_CONTEXT_RATIO
local COMPACTION_CONTEXT_LIMIT = MAX_CONTEXT_TOKENS * COMPACTION_CONTEXT_RATIO

local function all_tool_calls_completed()
    local context = mcl_command("GET req.ctx")
    local completed = {}
    for index = #context, 1, -1 do
        local current = context[index]
        if current.type == "tool" then
            completed[current.tool_call_id] = true
        elseif current.type == "assistant" then
            for _, call in ipairs(current.tool_calls or {}) do
                if not completed[call.id] then
                    return false
                end
            end
            return true
        else
            return false
        end
    end
    return false
end

local function maybe_compact(message)
    if message.type ~= "assistant" or not message.usage then
        return
    end
    local recent = mcl_command("GET msg.recent_conversation")
    local first_assistant_tokens
    for _, entry in ipairs(recent) do
        if entry.type == "assistant" and entry.usage then
            first_assistant_tokens = entry.usage.input_tokens
            break
        end
    end
    if first_assistant_tokens
        and message.usage.input_tokens - first_assistant_tokens >= RECENT_CONTEXT_LIMIT
    then
        move_old_recent_messages()
    end
    if message.usage.input_tokens >= COMPACTION_CONTEXT_LIMIT then
        move_old_recent_messages()
        local summary = mcl_command("EMIT EFFECT catch_inference FROM com")
        mcl_command("INJECT ? TO msg.compact_context", {
            type = "user",
            content = summary,
        })
        mcl_command("INJECT msg.recent_conversation TO msg.history_conversation")
        mcl_command("INJECT [] TO msg.recent_conversation")
        sync_realtime()
    end
end

while true do
    local message = mcl_command("EMIT EFFECT start")
    if message.type == "inject" then
        for _, injected in ipairs(message.messages or {}) do
            append_recent(injected)
        end
    elseif message.type == "user" then
        append_recent(message)
        mcl_command("EMIT EFFECT inference FROM req")
    elseif message.type == "assistant" then
        append_recent(message)
        maybe_compact(message)
        local tool_calls = message.tool_calls or {}
        if #tool_calls > 0 then
            mcl_command("EMIT EFFECT tool_call FROM ?", tool_calls)
        else
            mcl_command("EMIT EFFECT finish")
        end
    elseif message.type == "tool" then
        append_recent(message)
        if all_tool_calls_completed() then
            mcl_command("EMIT EFFECT inference FROM req")
        end
    elseif message.type == "error" then
        mcl_command("EMIT EFFECT history_append FROM ?", message)
    end
end
