function configure(arguments, context)
    return { timeout_ms = 30000 }
end

function execute(arguments, context)
    task_finish(arguments.task_id)
    task_return({ closed = true })
    task_finish(context.task_id)
end
