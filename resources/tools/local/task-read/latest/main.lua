function execute(arguments, context)
    local ok, result = pcall(task_read, arguments.task_id)
    if ok then
        task_return(result)
    else
        task_return({ ok = false, content = tostring(result) })
    end
    task_finish(context.task_id)
end
