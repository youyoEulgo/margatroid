-- edit
--
-- Replaces one exact string (or every occurrence of it) in one text file.
-- Absolute paths are used unchanged; relative paths are resolved against the
-- current agent's project root.

local MAX_FILE_BYTES = 16 * 1024 * 1024

local function is_absolute(path)
    local separator = package.config:sub(1, 1)
    if separator == "\\" then
        local drive = path:sub(1, 1):match("%a") ~= nil
            and path:sub(2, 2) == ":"
            and (path:sub(3, 3) == "\\" or path:sub(3, 3) == "/")
        return drive or path:sub(1, 1) == "\\"
    end
    return path:sub(1, 1) == "/"
end

local function resolve_path(path, root)
    if is_absolute(path) then return path end
    local separator = package.config:sub(1, 1)
    local last = root:sub(-1)
    if last == "/" or last == "\\" then return root .. path end
    return root .. separator .. path
end

local function read_text(path)
    local file, message = io.open(path, "rb")
    if not file then
        return nil, "cannot read " .. path .. ": " .. tostring(message)
    end
    local content, read_error = file:read(MAX_FILE_BYTES + 1)
    file:close()
    if content == nil then
        if read_error then return nil, "cannot read " .. path .. ": " .. tostring(read_error) end
        content = ""
    end
    if #content > MAX_FILE_BYTES then
        return nil, "file exceeds the size limit"
    end
    if not utf8.len(content) then return nil, "file is not valid UTF-8" end
    return content
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
    local path = resolve_path(arguments.path, context.project_root)
    local content, failure = read_text(path)
    if not content then
        return { ok = false, content = failure }
    end
    if arguments.old_string == "" then
        return { ok = false, content = "old_string must not be empty" }
    end
    if not utf8.len(arguments.old_string) or not utf8.len(arguments.new_string) then
        return { ok = false, content = "replacement strings must be valid UTF-8" }
    end
    local count, start = 0, 1
    while true do
        local first, last = content:find(arguments.old_string, start, true)
        if not first then break end
        count = count + 1
        if #content + count * (#arguments.new_string - #arguments.old_string) > MAX_FILE_BYTES then
            return { ok = false, content = "replacement exceeds the 16 MiB size limit" }
        end
        start = last + 1
        if count > 1 and not arguments.replace_all then
            return { ok = false, content = "old_string matched multiple locations; provide more context or set replace_all" }
        end
    end
    if count == 0 then return { ok = false, content = "old_string was not found in the file" } end
    local size = #content + count * (#arguments.new_string - #arguments.old_string)
    if size > MAX_FILE_BYTES then return { ok = false, content = "replacement exceeds the 16 MiB size limit" } end
    local literal_pattern = arguments.old_string:gsub("(%W)", "%%%1")
    local replacement = content:gsub(literal_pattern, function() return arguments.new_string end)
    local written, write_failure = write_text(path, replacement)
    if not written then
        return { ok = false, content = write_failure }
    end
    return {
        ok = true,
        content = string.format("Updated %s (%d replacement%s).", arguments.path, count, count == 1 and "" or "s"),
    }
end

function execute(arguments, context)
    local result = perform(arguments, context)
    task_return(result)
    task_finish(context.task_id)
end
