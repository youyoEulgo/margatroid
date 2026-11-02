-- list-directory
--
-- Lists the immediate children of one directory, sorted by name. Absolute paths
-- are used unchanged; relative paths are resolved against the current agent's
-- project root. Lua's standard library cannot read directories, so this uses
-- find; running programs is what the sandbox governs, not this file.

local MAX_OUTPUT_BYTES = 16 * 1024 * 1024

local function shell_quote(value)
    if package.config:sub(1, 1) == "\\" then
        return '"' .. value:gsub('"', '\\"') .. '"'
    end
    return "'" .. value:gsub("'", "'\\''") .. "'"
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

local function list_entries(path)
    local separator = package.config:sub(1, 1)
    local command = table.concat({
        "find", shell_quote(path), "-maxdepth", "1", "-mindepth", "1",
        "-printf", shell_quote("%y\t%f\\0"),
    }, " ")

    -- The host shell owns timeout and output budgets.
    local result = margatroid().shell(command, { max_output_bytes = MAX_OUTPUT_BYTES })
    if result.truncated then
        return nil, "directory listing exceeded the size limit"
    end
    if result.exit_code ~= 0 then
        return nil, "cannot list " .. path .. ": " .. result.stderr
    end
    local stdout = result.stdout

    local entries = {}
    for kind, name in stdout:gmatch("([^\t])\t([^%z]*)%z") do
        local mapped = "other"
        if kind == "f" then mapped = "file"
        elseif kind == "d" then mapped = "directory"
        elseif kind == "l" then mapped = "symlink"
        end
        entries[#entries + 1] = {
            name = name,
            path = path .. separator .. name,
            kind = mapped,
        }
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
    local entries, failure = list_entries(path)
    if not entries then
        return report(nil, failure)
    end
    return report(#entries == 0 and "[]" or margatroid().json.encode(entries))
end

function execute(arguments, context)
    local result = perform(arguments, context)
    task_return(result)
    task_finish(context.task_id)
end
