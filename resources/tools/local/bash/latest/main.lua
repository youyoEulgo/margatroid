function execute(arguments, context)
    local terminal = terminal_open()
    task_return({ task_id = context.task_id })
    while true do
        local source, value = await_any({
            output = function() return terminal:read() end,
            input = function() return task_receive() end,
        })
        if source == "output" then
            if value.eof then
                task_write({ exit_code = value.exit_code })
                break
            end
            if value.replace then
                task_replace(value.text)
            elseif value.text ~= "" then
                task_write(value.text)
            end
        else
            if type(value) ~= "table" or type(value.text) ~= "string" then
                task_write({ error = "input must be an object with text and optional enter" })
            else
                terminal:write(value.text .. (value.enter and "\r" or ""))
            end
        end
    end
    task_finish(context.task_id)
end
