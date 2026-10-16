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

function execute(arguments, context)
    local runtime = margatroid()
    local function failure(message)
        return runtime.json.encode({ok = false, content = message})
    end
    local path = resolve_path(arguments.path, context.project_root)
    local file = io.open(path, "rb")
    if not file then return failure("Cannot open image: " .. arguments.path) end
    local bytes = file:read(MAX_BYTES + 1)
    file:close()
    if not bytes then return failure("Cannot read image: " .. arguments.path) end
    if #bytes > MAX_BYTES then return failure("Image exceeds 5 MiB limit: " .. arguments.path) end
    local media = media_type(bytes)
    if not media then return failure("Not a supported PNG, JPEG, GIF or WebP image: " .. arguments.path) end
    return runtime.json.encode({
        ok = true,
        content = string.format("Image %s (%s, %d bytes)", arguments.path, media, #bytes),
        images = {{media_type = media, data = runtime.base64_encode(bytes)}}
    })
end
