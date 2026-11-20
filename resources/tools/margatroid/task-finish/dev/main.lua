function configure(arguments, context)
    return { timeout_ms = 30000 }
end

function execute(arguments, context)
    arguments = context.arguments
    margatroid.task:finish(arguments.task_id)
    margatroid.task:return_result({ closed = true })
    margatroid.task:finish(context.task_id)
end
