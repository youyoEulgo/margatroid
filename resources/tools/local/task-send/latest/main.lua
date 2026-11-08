function configure(arguments, context)
    return { timeout_ms = 30000 }
end

function execute(arguments, context)
    arguments = context.arguments
    margatroid.task:send(arguments.task_id, arguments.value)
    margatroid.task:return_result({ delivered = true })
    margatroid.task:finish(context.task_id)
end
