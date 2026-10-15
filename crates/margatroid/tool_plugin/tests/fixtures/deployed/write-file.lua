-- write-file
--
-- Replaces one text file. Absolute paths are used unchanged; relative paths are
-- resolved against the current agent's project root. Parent directories are only
-- created when the caller asks for it.

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

    local separator = package.config:sub(1, 1)
    local last = project_root:sub(-1)
    if last == "/" or last == "\\" then
        return project_root .. path
    end
    return project_root .. separator .. path
end

local function parent_path(path)
    local separator = package.config:sub(1, 1)
    local pattern = separator == "\\" and "[\\\\/]" or "[/]"
    local index = path:match("^.*()" .. pattern .. ".-$")
    if not index then
        return nil
    end
    return path:sub(1, index - 1)
end

local function create_directories(path)
    local ok, _, code = os.execute("mkdir -p " .. shell_quote(path))
    if ok ~= true and ok ~= 0 then
        return nil, "could not create " .. path .. " (exit " .. tostring(code) .. ")"
    end
    return true
end

local function write_text(path, content)
    local file, message = io.open(path, "wb")
    if not file then
        return nil, "cannot write " .. path .. ": " .. tostring(message)
    end
    file:write(content)
    file:close()
    return true
end

function execute(arguments, context)
    local path = resolve_path(arguments.path, context.project_root)
    if arguments.create_parent_directories then
        local parent = parent_path(path)
        if parent and parent ~= "" then
            local created, failure = create_directories(parent)
            if not created then
                return margatroid().json.encode({ ok = false, content = failure })
            end
        end
    end
    local written, failure = write_text(path, arguments.content)
    if not written then
        return margatroid().json.encode({ ok = false, content = failure })
    end
    return margatroid().json.encode({
        ok = true,
        content = margatroid().json.encode({
            path = path,
            bytes = #arguments.content,
            replaced = true,
        }),
    })
end
