function configure(arguments, context)
    return { timeout_ms = 30000 }
end

function execute(arguments, context)
    margatroid:task_return(margatroid:task_read(arguments.task_id))
    margatroid:task_finish(context.task_id)
end
