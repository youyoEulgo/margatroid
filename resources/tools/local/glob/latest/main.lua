function configure(arguments, context)
    return { timeout_ms = 30000 }
end

local function expected(message)
    error({ file_failure = true, message = message }, 0)
end
local function perform(arguments, context)
    local helper = assert(load(margatroid.files:read(context.resource_dir .. "/filewalk.lua"), "@filewalk.lua"))()
    local pattern = arguments.pattern
    local negate = pattern:sub(1, 1) == "!"
    if negate then pattern = pattern:sub(2) end
    margatroid.files:matches("", pattern)
    local paths, bytes = {}, 0
    local vcs = { [".git"] = true, [".hg"] = true, [".svn"] = true, [".jj"] = true }
    helper.walk(arguments.path or ".", { exclude = negate and pattern or nil }, function(info, path, explicit)
        if not explicit then
            for part in path:gmatch("([^/]+)/") do if vcs[part] then return end end
        end
        if explicit or (negate and pattern:sub(-1) == "/") or margatroid.files:matches(path:gsub("^%./", ""), pattern) ~= negate then
            if info.modified_at_ns == nil then expected("cannot obtain modification time for " .. path) end
            bytes = bytes + #path + 1
            if bytes > 16 * 1024 * 1024 then expected("glob output exceeded the size limit; narrow the pattern or path") end
            paths[#paths + 1] = { path = path, modified = info.modified_at_ns }
        end
    end)
    table.sort(paths, function(a, b)
        if a.modified ~= b.modified then return a.modified > b.modified end
        return a.path < b.path
    end)
    if #paths == 0 then return { ok = true, content = "No files found." } end
    local output = {}
    for index = 1, math.min(100, #paths) do
        output[#output + 1] = paths[index].path:gsub("\r", "\\r"):gsub("\n", "\\n")
    end
    if #paths > 100 then output[#output + 1] = string.format("[... %d additional files omitted; narrow the search ...]", #paths - 100) end
    return { ok = true, content = table.concat(output, "\n") }
end

function execute(arguments, context)
    arguments = context.arguments
    local ok, result = pcall(perform, arguments, context)
    if ok then margatroid.task:return_result(result)
    elseif type(result) == "table" and result.file_failure then
        margatroid.task:return_result({ ok = false, content = result.message })
    else error(result, 0) end
    margatroid.task:finish(context.task_id)
end
