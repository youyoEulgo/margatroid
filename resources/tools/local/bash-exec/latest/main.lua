function configure(arguments, context)
    return { timeout_ms = 120000 }
end

function execute(arguments, context)
    local quote = "'" .. arguments.cmd:gsub("'", "'\\''") .. "'"
    local client = dofile(context.resource_dir .. "/shell.lua")(context)
    local result = client.exec("bash --noprofile --norc -c " .. quote, { max_output_bytes = 64 * 1024 })
    local content = result.stdout
    if result.stderr ~= "" then
        if content ~= "" and content:sub(-1) ~= "\n" then content = content .. "\n" end
        content = content .. "[stderr]\n" .. result.stderr
    end
    if content == "" then
        content = "command completed with no output"
    end
    if result.truncated then content = content .. "\n[output truncated at the size limit]" end
    content = content .. "\n[exit code: " .. tostring(result.exit_code) .. "]"
    task_return({ ok = result.exit_code == 0, content = content })
    task_finish(context.task_id)
end
