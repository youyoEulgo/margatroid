function execute(arguments, context)
    task_send(arguments.task_id, arguments.value)
    task_return({ delivered = true })
    task_finish(context.task_id)
end
