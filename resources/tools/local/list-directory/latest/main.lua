-- list-directory
--
-- Lists the immediate children of one directory, sorted by name. Absolute paths
-- are used unchanged; relative paths are resolved against the current agent's
-- project root. Lua's standard library cannot read directories, so this uses
-- find; running programs is what the sandbox governs, not this file.

local MAX_OUTPUT_BYTES = 16 * 1024 * 1024

local function trim_newline(value)
    return value:gsub("[\r\n]+$", "")
end

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
        "-printf", shell_quote("%y\t%f\n"),
    }, " ")

    -- The listing runs through the shell the runner provides, so it gets a
    -- process group that is reclaimed on timeout and an execution budget.
    local result = margatroid().shell(command, { max_output_bytes = MAX_OUTPUT_BYTES })
    if result.truncated then
        return nil, "directory listing exceeded the size limit"
    end
    if result.exit_code ~= 0 then
        return nil, "cannot list " .. path
    end
    local stdout = result.stdout

    local entries = {}
    for kind, name in stdout:gmatch("([^\t])\t([^\n]*)\n") do
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

-- One envelope for every outcome: `content` is what the model reads, `ok` is
-- what tells the runtime and the interface whether the call failed.
local function report(content, failure)
    if failure then
        return margatroid().json.encode({ ok = false, content = failure })
    end
    return margatroid().json.encode({ ok = true, content = content })
end

function execute(arguments, context)
    local requested_path = arguments.path or "."
    local path = resolve_path(requested_path, context.project_root)
    local entries, failure = list_entries(path)
    if not entries then
        return report(nil, failure)
    end
    return report(margatroid().json.encode(entries))
end
