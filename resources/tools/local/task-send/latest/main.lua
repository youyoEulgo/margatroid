function configure(arguments, context)
    return { timeout_ms = 30000 }
end

function execute(arguments, context)
    margatroid:task_send(arguments.task_id, arguments.value)
    margatroid:task_return({ delivered = true })
    margatroid:task_finish(context.task_id)
end
