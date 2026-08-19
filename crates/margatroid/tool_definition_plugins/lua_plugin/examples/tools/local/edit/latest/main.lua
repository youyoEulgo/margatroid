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
    local content = margatroid.fs.read_text(path)
    local matches = find_matches(content, arguments.old_string)
    if #matches == 0 then
        error("old_string was not found in the file")
    end
    if not arguments.replace_all and #matches ~= 1 then
        error("old_string matched " .. #matches .. " locations; provide more context or set replace_all")
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
    margatroid.fs.write_text(path, replacement)
    return string.format("Updated %s (%d replacement%s).", arguments.path, count, count == 1 and "" or "s")
end
