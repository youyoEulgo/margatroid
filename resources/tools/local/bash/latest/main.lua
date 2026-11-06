function configure(arguments, context)
    return { timeout_ms = 0 }
end

function execute(arguments, context)
    local client = dofile(context.resource_dir .. "/shell.lua")(context)
    local terminal = client.terminal()
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
            if type(value) ~= "table" or type(value.text) ~= "string"
                or (value.enter ~= nil and type(value.enter) ~= "boolean") then
                task_write({ error = "input must be an object with string text and optional boolean enter" })
            else
                terminal:write(value.text .. (value.enter and "\r" or ""))
            end
        end
    end
    task_finish(context.task_id)
end
