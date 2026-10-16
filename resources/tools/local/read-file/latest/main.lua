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
    local file, message = io.open(path, "rb")
    if not file then
        return nil, "cannot read " .. path .. ": " .. tostring(message)
    end
    local content = file:read("*a")
    file:close()
    if content == nil then
        return nil, "cannot read " .. path
    end
    if #content > MAX_FILE_BYTES then
        return nil, "file exceeds the size limit"
    end
    return content
end

function execute(arguments, context)
    local path = resolve_path(arguments.path, context.project_root)
    local offset = arguments.offset or 1
    local limit = arguments.limit or 2000
    local content, failure = read_text(path)
    if not content then
        return margatroid().json.encode({ ok = false, content = failure })
    end

    local lines = {}
    local start = 1
    while true do
        local newline = content:find("\n", start, true)
        if newline then
            lines[#lines + 1] = content:sub(start, newline - 1)
            start = newline + 1
        else
            if start <= #content then
                lines[#lines + 1] = content:sub(start)
            elseif #content == 0 then
                lines[1] = ""
            end
            break
        end
    end

    local output = {}
    local last = math.min(#lines, offset + limit - 1)
    for line_number = offset, last do
        output[#output + 1] = string.format("%6d\t%s", line_number, lines[line_number])
    end
    local header = string.format("%s (lines %d-%d of %d)", arguments.path, offset, last, #lines)
    if offset > #lines then
        return margatroid().json.encode({ ok = true, content = header .. "\n(end of file)" })
    end
    return margatroid().json.encode({ ok = true, content = header .. "\n" .. table.concat(output, "\n") })
end
