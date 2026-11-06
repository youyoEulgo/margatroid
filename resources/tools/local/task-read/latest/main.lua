function configure(arguments, context)
    return { timeout_ms = 30000 }
end

function execute(arguments, context)
    task_return(task_read(arguments.task_id))
    task_finish(context.task_id)
end
