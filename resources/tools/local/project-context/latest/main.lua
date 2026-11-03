function configure(arguments, context)
    return { timeout_ms = 30000 }
end

function execute(arguments, context)
    task_return([=[
# Home Project Context Guidance

Read the project structure and local design documents before taking an action.

(〜￣д￣)〜
]=])
    task_finish(context.task_id)
end
