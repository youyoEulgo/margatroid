local function perform(arguments, context)
    return [=[
# Home Code Review Guidance

Review code for correctness, regressions, and missing tests. Lead with concrete findings.
]=]
end

function execute(arguments, context)
    local result = perform(arguments, context)
    if type(result) == "string" then
        local ok, value = pcall(margatroid().json.decode, result)
        if ok and type(value) == "table" and type(value.ok) == "boolean" then
            result = value
        end
    end
    task_return(result)
    task_finish(context.task_id)
end
