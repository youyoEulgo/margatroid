function configure(arguments, context)
    return { timeout_ms = 0 }
end

function execute(arguments, context)
    arguments = context.arguments
    local client = dofile(context.resource_dir .. "/shell.lua")(context)
    local terminal = client.terminal()
    margatroid.task:return_result({ task_id = context.task_id })
    while true do
        local source, value = margatroid:wait_any({
            output = function() return terminal:read() end,
            input = function() return margatroid.task:receive() end,
        })
        if source == "output" then
            if value.eof then
                margatroid.task:append_output({ exit_code = value.exit_code })
                break
            end
            if value.replace then
                margatroid.task:replace_output(value.text)
            elseif value.text ~= "" then
                margatroid.task:append_output(value.text)
            end
        else
            if type(value) ~= "table" or type(value.text) ~= "string"
                or (value.enter ~= nil and type(value.enter) ~= "boolean") then
                margatroid.task:append_output({ error = "input must be an object with string text and optional boolean enter" })
            else
                terminal:write(value.text .. (value.enter and "\r" or ""))
            end
        end
    end
    margatroid.task:finish(context.task_id)
end
