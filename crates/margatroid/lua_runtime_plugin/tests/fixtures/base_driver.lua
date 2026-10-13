local function command(text, binding)
    return mcl(agent_info.id, text, binding)
end

command("IMPORT MESSAGE prompt:system/soul:latest AS soul")
command("CREATE BLOCK msg (system_prompt MESSAGE, recent_conversation MESSAGE)")
command("CREATE BLOCK req (ctx MESSAGE)")
command("CREATE BLOCK entry (pending MESSAGE)")
command("INJECT soul TO msg.system_prompt")
command("EMIT EFFECT ready")

while true do
    local message = command("GET entry.pending[0]")
    if message then
        command("INJECT ? TO entry.pending[0, 1]", {})
        if message.type == "user" then
            command("EMIT EFFECT start_turn")
            command("INJECT msg.system_prompt, msg.recent_conversation TO req.ctx")
            local reply = command("EMIT EFFECT inference FROM req")
            command("EMIT EFFECT history_append FROM ?", { reply })
            command("EMIT EFFECT finish")
        end
    else
        wait()
    end
end
