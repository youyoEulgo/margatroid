local function command(text, arguments)
    if arguments == nil then
        return mcl(agent_info.id, text)
    end
    return mcl(agent_info.id, text, arguments)
end

command("IMPORT MESSAGE prompt:system/soul:latest AS soul")
command("CREATE TEMP BLOCK msg (system_prompt MESSAGE)")
local session = command("CREATE BLOCK session (conversation MESSAGE, visible TOOL)")
command("CREATE TEMP BLOCK entry (pending MESSAGE)")
command("INJECT soul TO msg.system_prompt")
if session.created then
    command("INJECT [] TO session.conversation")
    command("INJECT [] TO session.visible")
end
command("EMIT EFFECT ready")

while true do
    local pending = command("GET entry.pending[0]")
    if #pending > 0 then
        command("INJECT [] TO entry.pending[0,1]")
        for _, message in ipairs(pending) do
            if message.type == "user" then
                command("EMIT EFFECT start_turn")
                command("INJECT ? TO session.conversation[-1]", {{message}})
                local reply = command("EMIT EFFECT inference (msg.system_prompt, session.conversation) (session.visible)")
                command("EMIT EFFECT history_append ?", {{reply.message}})
                command("EMIT EFFECT finish_turn")
            end
        end
    else
        wait()
    end
end
