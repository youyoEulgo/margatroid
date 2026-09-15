-- Base Driver for every Agent image.
-- `mcl` is the host boundary injected by Margatroid. The block layout,
-- message routing, pending tool state, and effect loop are all defined here.

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
        pending_tool TOOL_CALL,
    )
]])

mcl_command([[
    CREATE BLOCK tool (
        tool_default TOOL,
        tool_dynamic TOOL,
    )
]])

mcl_command([[
    CREATE REF_BLOCK req (
        REF_MERGE system_prompt, compact_context, history_conversation, recent_conversation FROM msg AS ctx,
        REF_MERGE tool_dynamic FROM tool AS vis,
    )
]])

mcl_command([[
    CREATE REF_BLOCK realtime (
        REF_MERGE compact_context, history_conversation, recent_conversation FROM msg AS ctx,
    )
]])

mcl_command([[
    CREATE REF_BLOCK com (
        REF_MERGE system_prompt, compact_context, history_conversation, compact_prompt FROM msg AS ctx,
    )
]])

mcl_command("INJECT soul TO system_prompt FROM msg")
mcl_command("INJECT compact TO compact_prompt FROM msg")
mcl_command("INJECT read_file, write_file, edit, grep, glob, bash, list_directory TO tool_default FROM tool")
mcl_command("INJECT SELECT tool_default FROM tool COVER tool_dynamic FROM tool")
mcl_command("EMIT EFFECT visibility_source (SELECT tool_dynamic FROM tool)")
mcl_command("EMIT EFFECT default_visibility_source (SELECT tool_default FROM tool)")

local function append_recent(message)
    mcl_command("INJECT ? TO recent_conversation FROM msg", message)
    mcl_command("EMIT EFFECT history_append", message)
end

local function move_old_recent_messages()
    local assistant_seen = false
    while true do
        local recent = mcl_command("SELECT recent_conversation FROM msg")
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
        mcl_command("INJECT ? TO history_conversation FROM msg", first)
        mcl_command("DELETE recent_conversation FIRST FROM msg")
    end
end

local restored = mcl_command("EMIT EFFECT realtime_load")
for _, message in ipairs(restored) do
    mcl_command("INJECT ? TO recent_conversation FROM msg", message)
end
move_old_recent_messages()
mcl_command("EMIT EFFECT realtime_source (realtime)")

local MAX_CONTEXT_TOKENS = agent_info.model.context_window_tokens
local RECENT_CONTEXT_RATIO = 0.16
local COMPACTION_CONTEXT_RATIO = 0.80
local RECENT_CONTEXT_LIMIT = MAX_CONTEXT_TOKENS * RECENT_CONTEXT_RATIO
local COMPACTION_CONTEXT_LIMIT = MAX_CONTEXT_TOKENS * COMPACTION_CONTEXT_RATIO

local function maybe_compact(message)
    if message.type ~= "assistant"
        or not message.usage
        or #(message.tool_calls or {}) > 0
    then
        return
    end
    local recent = mcl_command("SELECT recent_conversation FROM msg")
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
        local summary = mcl_command("EMIT EFFECT catch_inference (com)")
        -- Replace the compressed state, then promote the current recent window.
        mcl_command("INJECT ? COVER compact_context FROM msg", {
            type = "user",
            content = summary,
        })
        mcl_command("INJECT SELECT recent_conversation COVER history_conversation FROM msg")
        mcl_command("DELETE recent_conversation FROM msg")
    end
end

while true do
    local message = mcl_command("EMIT EFFECT start")

    if message.type == "user" then
        append_recent(message)
        mcl_command("EMIT EFFECT inference (req)")

    elseif message.type == "assistant" then
        append_recent(message)
        maybe_compact(message)
        local tool_calls = message.tool_calls or {}
        if #tool_calls > 0 then
            for _, tool_call in ipairs(tool_calls) do
                mcl_command("INJECT ? TO pending_tool FROM msg", tool_call)
            end
            mcl_command("EMIT EFFECT tool_call ?", tool_calls)
        else
            mcl_command("EMIT EFFECT finish")
        end

    elseif message.type == "tool" then
        append_recent(message)
        mcl_command("DELETE pending_tool FROM msg WHERE id == ?", message.tool_call_id)
        local pending_tools = mcl_command("SELECT pending_tool FROM msg")
        if #pending_tools == 0 then
            mcl_command("EMIT EFFECT inference (req)")
        end

    elseif message.type == "error" then
        mcl_command("EMIT EFFECT history_append", message)
    end
end
