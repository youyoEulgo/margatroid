function configure()
    return { timeout_ms = 30000 }
end

function execute(arguments, context)
    local tasks = margatroid.task:list()
    -- Preserve the empty JSON array kind supplied by the host.
    margatroid.task:return_result({tasks = tasks})
    margatroid.task:finish(context.task_id)
end
