function execute(arguments, context)
    task_finish(arguments.task_id)
    task_return({ closed = true })
    task_finish(context.task_id)
end
