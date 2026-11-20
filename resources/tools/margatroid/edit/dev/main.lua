function configure(arguments, context)
    return { timeout_ms = 30000 }
end

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

local function write_text(path, content)
    margatroid.files:write(path, content)
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
    arguments = context.arguments
    local result = perform(arguments, context)
    margatroid.task:return_result(result)
    margatroid.task:finish(context.task_id)
end
