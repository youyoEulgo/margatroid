function configure(arguments, context)
    return { timeout_ms = 30000 }
end

function execute(arguments, context)
    arguments = context.arguments
    margatroid.task:return_result(margatroid.task:read_output(arguments.task_id))
    margatroid.task:finish(context.task_id)
end
