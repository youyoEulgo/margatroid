function configure(arguments, context)
    return { timeout_ms = 30000 }
end

-- read-file
--
-- Reads a numbered window from one text file. Absolute paths are used unchanged;
-- relative paths are resolved against the current agent's project root.
--
-- Tools are trusted code: parent components such as "../" and absolute paths are
-- used as given. What a tool may actually touch is decided by the process sandbox,
-- not by this resolution.

local MAX_FILE_BYTES = 16 * 1024 * 1024

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

    local separator = package.config:sub(1, 1)
    local last = project_root:sub(-1)
    if last == "/" or last == "\\" then
        return project_root .. path
    end
    return project_root .. separator .. path
end

local function read_text(path)
    local info = margatroid.files:info(path)
    if info.kind == "symlink" and info.resolved_path then info = margatroid.files:info(info.resolved_path) end
    if info.kind == "file" and info.size > MAX_FILE_BYTES then return nil, "file exceeds the size limit" end
    local content = margatroid.files:read(path)
    if #content > MAX_FILE_BYTES then
        return nil, "file exceeds the size limit"
    end
    if not utf8.len(content) then return nil, "file is not valid UTF-8" end
    return content
end

local function perform(arguments, context)
    local path = resolve_path(arguments.path, context.project_root)
    local offset = arguments.offset or 1
    local limit = arguments.limit or 2000
    local content, failure = read_text(path)
    if not content then
        return { ok = false, content = failure }
    end

    local output = {}
    local total, start, output_bytes = 0, 1, 0
    while start <= #content do
        local newline = content:find("\n", start, true)
        local last = newline and newline - 1 or #content
        total = total + 1
        if total >= offset and total - offset < limit then
            local line = content:sub(start, last):gsub("\r$", "")
            local numbered = string.format("%6d\t%s", total, line)
            output_bytes = output_bytes + #numbered + 1
            if output_bytes > 256 * 1024 then
                return { ok = false, content = "requested lines exceed the 256 KiB output limit; reduce limit or select shorter lines" }
            end
            output[#output + 1] = numbered
        end
        if not newline then break end
        start = newline + 1
    end
    if offset > total then
        return { ok = true, content = string.format("%s (%d lines)\n(end of file)", arguments.path, total) }
    end
    local last = offset + #output - 1
    local header = string.format("%s (lines %d-%d of %d)", arguments.path, offset, last, total)
    return { ok = true, content = header .. "\n" .. table.concat(output, "\n") }

end

function execute(arguments, context)
    arguments = context.arguments
    local result = perform(arguments, context)
    margatroid.task:return_result(result)
    margatroid.task:finish(context.task_id)
end
