function execute(arguments, context)
    local result = task_read(arguments.task_id)
    task_return(result)
    task_finish(context.task_id)
end
