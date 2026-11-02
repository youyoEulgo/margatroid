function execute(arguments, context)
    local ok, result = pcall(task_finish, arguments.task_id)
    if ok then
        task_return({ closed = true })
    else
        task_return({ ok = false, content = tostring(result) })
    end
    task_finish(context.task_id)
end
