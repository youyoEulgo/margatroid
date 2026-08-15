-- read-file
--
-- Reads one UTF-8 text file and returns its complete contents as the tool
-- response. Absolute paths are used unchanged. Relative paths are resolved
-- against the current Agent's project root, making them independent of the
-- daemon process working directory.
--
-- Lua tools are trusted code. Resolution intentionally permits parent path
-- components such as "../" and does not confine absolute paths to the project.
--
-- Margatroid calls this function with:
--   arguments: values validated against input.schema.json
--   context:   read-only metadata for this agent turn and tool package

-- Determine whether a path is absolute using the host platform's directory
-- separator. package.config is part of Lua's standard library and begins with
-- that separator: "/" on Unix-like systems and "\\" on Windows.
local function is_absolute(path)
    local separator = package.config:sub(1, 1)

    if separator == "\\" then
        -- Accept drive-qualified paths such as C:\\work\\file.txt and paths
        -- rooted at the current drive or a UNC share.
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

function execute(arguments, context)
    local path = resolve_path(arguments.path, context.project_root)

    -- read_text is an asynchronous Rust host function. mlua suspends the Lua
    -- coroutine while the file is read, so Lua code can use it like a normal
    -- function without an explicit await operation.
    --
    -- The host API rejects non-UTF-8 data, unreadable paths, directories, and
    -- files exceeding the configured Lua output-size limit. Such failures are
    -- returned by LuaPlugin as failed tool responses.
    local content = margatroid.fs.read_text(path)

    -- A Lua tool must return a UTF-8 string. Returning the file contents
    -- directly preserves whitespace and avoids adding explanatory text that
    -- was not present in the source file.
    return content
end
