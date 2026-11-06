function configure(arguments, context)
    return { timeout_ms = 30000 }
end

local MAX_FILE_BYTES = 16 * 1024 * 1024

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

local function create_directories(path, context)
    local client = dofile(context.resource_dir .. "/shell.lua")(context)
    local result = client.exec("mkdir -p -- " .. shell_quote(path), { max_output_bytes = 4096 })
    if result.exit_code ~= 0 then
        return nil, "could not create " .. path .. ": " .. result.stderr
    end
    return true
end

local function write_text(path, content)
    local file, message = io.open(path, "wb")
    if not file then
        return nil, "cannot write " .. path .. ": " .. tostring(message)
    end
    local written, write_error = file:write(content)
    local closed, close_error = file:close()
    if not written then return nil, "cannot write " .. path .. ": " .. tostring(write_error) end
    if not closed then return nil, "cannot close " .. path .. ": " .. tostring(close_error) end
    return true
end

local function perform(arguments, context)
    if #arguments.content > MAX_FILE_BYTES then return { ok = false, content = "content exceeds the 16 MiB size limit" } end
    if not utf8.len(arguments.content) then return { ok = false, content = "content is not valid UTF-8" } end
    local path = resolve_path(arguments.path, context.project_root)
    if arguments.create_parent_directories then
        local parent = parent_path(path)
        if parent and parent ~= "" then
            local created, failure = create_directories(parent, context)
            if not created then
                return { ok = false, content = failure }
            end
        end
    end
    local written, failure = write_text(path, arguments.content)
    if not written then
        return { ok = false, content = failure }
    end
    return {
        ok = true,
        content = json.encode({
            path = path,
            bytes = #arguments.content,
            replaced = true,
        }),
    }
end

function execute(arguments, context)
    local result = perform(arguments, context)
    task_return(result)
    task_finish(context.task_id)
end
