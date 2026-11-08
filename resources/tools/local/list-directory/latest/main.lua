function configure(arguments, context)
    return { timeout_ms = 30000 }
end

local function is_absolute(path)
    local separator = package.config:sub(1, 1)

    if separator == "\\" then
        local drive_qualified = path:sub(1, 1):match("%a") ~= nil
            and path:sub(2, 2) == ":"
            and (path:sub(3, 3) == "\\" or path:sub(3, 3) == "/")
        return drive_qualified or path:sub(1, 1) == "\\"
    end

    return path:sub(1, 1) == "/"
end

local function resolve_path(path, project_root)
    if is_absolute(path) then
        return path
    end

    if path == "." then
        return project_root
    end

    local separator = package.config:sub(1, 1)
    local last = project_root:sub(-1)
    if last == "/" or last == "\\" then
        return project_root .. path
    end
    return project_root .. separator .. path
end

local function list_entries(path, context)
    local children = margatroid.files:list(path)
    local entries, bytes = {}, 0
    for _, child in ipairs(children) do
        bytes = bytes + #child.name + 3 -- Original kind, tab, name, NUL wire budget.
        if bytes > 16 * 1024 * 1024 then return nil, "directory listing exceeded the size limit" end
        entries[#entries + 1] = { name = child.name, path = child.path, kind = child.kind }
    end
    table.sort(entries, function(left, right) return left.name < right.name end)
    return entries
end

local function report(content, failure)
    if failure then
        return { ok = false, content = failure }
    end
    return { ok = true, content = content }
end

local function perform(arguments, context)
    local requested_path = arguments.path or "."
    local path = resolve_path(requested_path, context.project_root)
    local entries, failure = list_entries(path, context)
    if not entries then
        return report(nil, failure)
    end
    return report(#entries == 0 and "[]" or margatroid.data:encode("json", entries))
end

function execute(arguments, context)
    arguments = context.arguments
    local result = perform(arguments, context)
    margatroid.task:return_result(result)
    margatroid.task:finish(context.task_id)
end
