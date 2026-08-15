-- list-directory
--
-- Lists the immediate children of one directory. Absolute paths are used
-- unchanged. Relative paths are resolved against the current Agent's project
-- root, making the result independent of the daemon process working directory.
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
        -- Accept drive-qualified paths such as C:\\work and paths rooted at
        -- the current drive or a UNC share.
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

function execute(arguments, context)
    -- JSON Schema defaults describe behavior to the model but are not inserted
    -- into the argument object by Margatroid, so the Lua implementation applies
    -- the default explicitly.
    local requested_path = arguments.path or "."
    local path = resolve_path(requested_path, context.project_root)

    -- list is an asynchronous Rust host function. It reads one directory level
    -- and returns entries already sorted by name. Each entry has:
    --   name: file name without its parent path
    --   path: resolved path reported by the operating system
    --   kind: "file", "directory", "symlink", or "other"
    local entries = margatroid.fs.list(path)

    -- A Lua tool must return a UTF-8 string. Encode the structured result as
    -- compact JSON so the model can reliably distinguish names and kinds.
    return margatroid.json.encode(entries)
end
