function configure(arguments, context)
    return { timeout_ms = 0 }
end

function execute(arguments, context)
    local client = dofile(context.resource_dir .. "/shell.lua")(context)
    local terminal = client.terminal()
    margatroid:task_return({ task_id = context.task_id })
    while true do
        local source, value = margatroid:await_any({
            output = function() return terminal:read() end,
            input = function() return margatroid:task_receive() end,
        })
        if source == "output" then
            if value.eof then
                margatroid:task_write({ exit_code = value.exit_code })
                break
            end
            if value.replace then
                margatroid:task_replace(value.text)
            elseif value.text ~= "" then
                margatroid:task_write(value.text)
            end
        else
            if type(value) ~= "table" or type(value.text) ~= "string"
                or (value.enter ~= nil and type(value.enter) ~= "boolean") then
                margatroid:task_write({ error = "input must be an object with string text and optional boolean enter" })
            else
                terminal:write(value.text .. (value.enter and "\r" or ""))
            end
        end
    end
    margatroid:task_finish(context.task_id)
end
