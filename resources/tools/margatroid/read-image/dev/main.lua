function configure(arguments, context)
    return { timeout_ms = 30000 }
end

local MAX_BYTES = 5 * 1024 * 1024

local function resolve_path(path, root)
    local separator = package.config:sub(1, 1)
    local absolute = path:sub(1, 1) == "/"
    if separator == "\\" then
        absolute = path:sub(1, 1) == "\\" or path:match("^%a:[/\\]") ~= nil
    end
    if absolute then return path end
    if root:sub(-1) == "/" or root:sub(-1) == "\\" then return root .. path end
    return root .. separator .. path
end

local function media_type(bytes)
    if bytes:sub(1, 8) == "\137PNG\r\n\26\n" then return "image/png" end
    if bytes:sub(1, 3) == "\255\216\255" then return "image/jpeg" end
    if bytes:sub(1, 6) == "GIF87a" or bytes:sub(1, 6) == "GIF89a" then return "image/gif" end
    if bytes:sub(1, 4) == "RIFF" and bytes:sub(9, 12) == "WEBP" then return "image/webp" end
end

local function perform(arguments, context)
    local function failure(message)
        return {ok = false, content = message}
    end
    local path = resolve_path(arguments.path, context.project_root)
    local info = margatroid.files:info(path)
    if info.kind == "symlink" and info.resolved_path then info = margatroid.files:info(info.resolved_path) end
    if info.kind == "file" and info.size > MAX_BYTES then return failure("Image exceeds 5 MiB limit: " .. arguments.path) end
    local bytes = margatroid.files:read(path)
    if #bytes == 0 then return failure("Image is empty or unreadable: " .. arguments.path) end
    if #bytes > MAX_BYTES then return failure("Image exceeds 5 MiB limit: " .. arguments.path) end
    local media = media_type(bytes)
    if not media then return failure("Not a supported PNG, JPEG, GIF or WebP image: " .. arguments.path) end
    return {
        ok = true,
        content = string.format("Image %s (%s, %d bytes)", arguments.path, media, #bytes),
        images = {{media_type = media, data = margatroid.data:encode("base64", bytes)}}
    }
end

function execute(arguments, context)
    arguments = context.arguments
    local result = perform(arguments, context)
    margatroid.task:return_result(result)
    margatroid.task:finish(context.task_id)
end
