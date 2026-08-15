-- write-file
--
-- Replaces one UTF-8 text file. Absolute paths are used unchanged. Relative
-- paths are resolved against the current Agent's project root.
--
-- Lua tools are trusted code. Resolution intentionally permits parent path
-- components such as "../" and does not confine absolute paths to the project.

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
    local index = path:match("^.*()" .. (separator == "\\" and "[\\\\/]" or "[/]") .. ".-$")
    if not index then
        return nil
    end
    return path:sub(1, index - 1)
end

function execute(arguments, context)
    local path = resolve_path(arguments.path, context.project_root)
    if arguments.create_parent_directories then
        local parent = parent_path(path)
        if parent and parent ~= "" then
            margatroid.fs.create_dir_all(parent)
        end
    end
    margatroid.fs.write_text(path, arguments.content)
    return margatroid.json.encode({
        path = path,
        bytes = #arguments.content,
        replaced = true
    })
end
