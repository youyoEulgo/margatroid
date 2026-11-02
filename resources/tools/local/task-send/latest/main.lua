function execute(arguments, context)
    local ok, result = pcall(task_send, arguments.task_id, arguments.value)
    if ok then
        task_return({ delivered = true })
    else
        task_return({ ok = false, content = tostring(result) })
    end
    task_finish(context.task_id)
end
