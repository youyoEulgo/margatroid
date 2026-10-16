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
    local content = file:read("*a")
    file:close()
    if content == nil then
        return nil, "cannot read " .. path
    end
    if #content > MAX_FILE_BYTES then
        return nil, "file exceeds the size limit"
    end
    return content
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

local function find_matches(content, needle)
    local positions = {}
    local start = 1
    while true do
        local first, last = content:find(needle, start, true)
        if not first then break end
        positions[#positions + 1] = { first = first, last = last }
        start = last + 1
    end
    return positions
end

function execute(arguments, context)
    local path = resolve_path(arguments.path, context.project_root)
    local content, failure = read_text(path)
    if not content then
        return margatroid().json.encode({ ok = false, content = failure })
    end
    local matches = find_matches(content, arguments.old_string)
    if #matches == 0 then
        return margatroid().json.encode({ ok = false, content = "old_string was not found in the file" })
    end
    if not arguments.replace_all and #matches ~= 1 then
        return margatroid().json.encode({
            ok = false,
            content = "old_string matched " .. #matches .. " locations; provide more context or set replace_all",
        })
    end

    local replacement
    local count
    if arguments.replace_all then
        local pieces = {}
        local start = 1
        for _, match in ipairs(matches) do
            pieces[#pieces + 1] = content:sub(start, match.first - 1)
            pieces[#pieces + 1] = arguments.new_string
            start = match.last + 1
        end
        pieces[#pieces + 1] = content:sub(start)
        replacement = table.concat(pieces)
        count = #matches
    else
        local match = matches[1]
        replacement = content:sub(1, match.first - 1)
            .. arguments.new_string
            .. content:sub(match.last + 1)
        count = 1
    end
    local written, write_failure = write_text(path, replacement)
    if not written then
        return margatroid().json.encode({ ok = false, content = write_failure })
    end
    return margatroid().json.encode({
        ok = true,
        content = string.format("Updated %s (%d replacement%s).", arguments.path, count, count == 1 and "" or "s"),
    })
end
