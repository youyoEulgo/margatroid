function configure(arguments, context)
    return { timeout_ms = 30000 }
end

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

local function perform(arguments, context)
    local function failure(message)
        return {ok = false, content = message}
    end
    local path = resolve_path(arguments.path, context.project_root)
    local bytes = margatroid.files:read(path)
    if #bytes == 0 then return failure("Image is empty or unreadable: " .. arguments.path) end
    local prepared, preview = pcall(function()
        return margatroid.data:image_prepare(bytes, {
            max_dimension = arguments.max_dimension or 1280,
            target_bytes = arguments.target_bytes or 300 * 1024,
            quality = arguments.quality or 80,
        })
    end)
    if not prepared then return failure("Image preview could not be prepared: " .. tostring(preview)) end
    return {
        ok = true,
        content = string.format("Image preview loaded for your visual inspection: %s. Original %dx%d, %d bytes; preview %dx%d, %s, %d bytes. Aspect ratio preserved; source file unchanged. Animated images use the first frame. Inspect the attached image and answer the user's request; loading it does not deliver it to the user. Request a larger max_dimension, target_bytes or quality if fine details are unclear.", arguments.path, preview.original_width, preview.original_height, preview.original_bytes, preview.width, preview.height, preview.media_type, #preview.data),
        images = {{media_type = preview.media_type, data = margatroid.data:encode("base64", preview.data)}}
    }
end

function execute(arguments, context)
    arguments = context.arguments
    local result = perform(arguments, context)
    margatroid.task:return_result(result)
    margatroid.task:finish(context.task_id)
end
