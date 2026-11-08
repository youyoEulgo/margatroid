function configure(arguments, context)
    return { timeout_ms = 30000 }
end

function execute(arguments, context)
    margatroid:task_finish(arguments.task_id)
    margatroid:task_return({ closed = true })
    margatroid:task_finish(context.task_id)
end
