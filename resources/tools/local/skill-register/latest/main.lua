-- Skill policy lives in Lua; the host supplies generic filesystem and YAML IO.
local TEXT_BYTES = 8 * 1024 * 1024
local MAX_ENTRIES = 10000
local WINDOWS = false

function configure(arguments, context)
    return { timeout_ms = 30000 }
end

local function normalized(path)
    return WINDOWS and path:gsub("\\", "/") or path
end

local function basename(path)
    return normalized(path):match("([^/]+)/*$")
end

local function parent(path)
    return normalized(path):match("^(.*)/[^/]+/*$") or "."
end

local function join(root, path)
    return root:gsub("/$", "") .. "/" .. path
end

local function components(path)
    local parts = {}
    for part in normalized(path):gmatch("[^/]+") do parts[#parts + 1] = part end
    return parts
end

local function contains(root, path)
    local base, target = components(root), components(path)
    if #target < #base then return false end
    for index, part in ipairs(base) do
        if target[index] ~= part then return false end
    end
    return true
end

local function canonical(path)
    local ok, actual = pcall(function() return margatroid:canonicalize(path) end)
    if not ok then error("cannot resolve " .. path .. ": " .. tostring(actual)) end
    return normalized(actual)
end

local function root_for(skill_md)
    if basename(skill_md) ~= "SKILL.md" then error("path must name SKILL.md") end
    local root, actual = canonical(parent(skill_md)), canonical(skill_md)
    if not contains(root, actual) then error("SKILL.md resolves outside its root") end
    if parent(actual) ~= root then error("SKILL.md must resolve directly inside its skill root") end
    if basename(actual) ~= "SKILL.md" then error("canonical file must name SKILL.md") end
    return root, actual
end

local function read_bytes(path, root)
    local actual = canonical(path)
    if not contains(root, actual) then error("file resolves outside skill root: " .. path) end
    local info = margatroid:metadata(actual)
    if info.kind ~= "file" then error("only regular files can be read: " .. path) end
    -- read_file has no prefix argument. Refuse oversized resources before IO,
    -- including binary resources, rather than read a huge file to classify it.
    if info.size_bytes > TEXT_BYTES then error("file exceeds 8388608 byte read budget: " .. path) end
    local bytes = margatroid:read_file(actual)
    if #bytes > TEXT_BYTES then error("file exceeds 8388608 byte read budget: " .. path) end
    return actual, bytes, info.size_bytes
end

local function object(value)
    return value ~= margatroid.null and type(value) == "table"
        and margatroid:json_encode(value):sub(1, 1) == "{"
end

local function nonblank(value)
    for _, code in utf8.codes(value) do
        local space = (code >= 9 and code <= 13) or code == 32 or code == 133
            or code == 160 or code == 5760 or (code >= 8192 and code <= 8202)
            or code == 8232 or code == 8233 or code == 8239 or code == 8287 or code == 12288
        if not space then return true end
    end
    return false
end

local function metadata(text, root)
    if not utf8.len(text) or text:find("\0", 1, true) then error("SKILL.md must be UTF-8 text") end
    text = text:gsub("^\239\187\191", "")
    local first, start = text:match("^([^\n]*)\n()")
    if not first or first:gsub("%s+$", "") ~= "---" then
        error("SKILL.md must start with YAML frontmatter delimiter ---")
    end
    local lines, closed = {}, false
    while start <= #text do
        local newline = text:find("\n", start, true)
        local line = text:sub(start, newline and newline - 1 or #text)
        if line:gsub("%s+$", "") == "---" then closed = true; break end
        lines[#lines + 1] = line
        if not newline then break end
        start = newline + 1
    end
    if not closed then error("YAML frontmatter is missing closing ---") end
    local ok, meta = pcall(function() return margatroid:yaml_decode(table.concat(lines, "\n") .. "\n") end)
    if not ok then error("invalid skill frontmatter: " .. tostring(meta)) end
    if not object(meta) then error("skill frontmatter must be a YAML mapping") end
    local known = { name = true, description = true, license = true,
        compatibility = true, metadata = true, ["allowed-tools"] = true }
    for key, value in pairs(meta) do
        if type(key) ~= "string" or not known[key] then
            error("invalid skill frontmatter field: " .. tostring(key))
        end
        if key == "metadata" then
            if not object(value) then error("metadata must map strings to strings") end
            for field, item in pairs(value) do
                if type(field) ~= "string" or type(item) ~= "string" then
                    error("metadata must map strings to strings")
                end
            end
        elseif type(value) ~= "string" then
            error("frontmatter field " .. key .. " must be a string")
        end
    end
    local name = meta.name
    if type(name) ~= "string" or #name == 0 or #name > 64
        or name:find("[^a-z0-9-]") or name:sub(1, 1) == "-"
        or name:sub(-1) == "-" or name:find("--", 1, true) then
        error("name must be 1–64 lowercase ASCII letters, digits or single interior hyphens")
    end
    if basename(root) ~= name then error("name must match the skill directory name") end
    if type(meta.description) ~= "string" or not nonblank(meta.description)
        or utf8.len(meta.description) > 1024 then
        error("description must contain 1–1024 characters")
    end
    if meta.compatibility ~= nil and (not nonblank(meta.compatibility)
        or utf8.len(meta.compatibility) > 500) then
        error("compatibility must contain 1–500 characters")
    end
    return meta
end

local function inventory(root)
    local files, ancestors, visited = {}, {}, 0
    local function walk(dir, relative)
        local actual = canonical(dir)
        if not contains(root, actual) then error("directory symlink resolves outside skill root") end
        if ancestors[actual] then error("directory symlink cycle in skill") end
        ancestors[actual] = true
        local children = margatroid:read_dir(dir)
        table.sort(children, function(left, right) return left.path < right.path end)
        for _, child in ipairs(children) do
            visited = visited + 1
            if visited > MAX_ENTRIES then error("inventory exceeds 10000 filesystem entries") end
            local path = child.path
            local target = canonical(path)
            if not contains(root, target) then error("symlink resolves outside skill root: " .. path) end
            local info = margatroid:metadata(target)
            local rel = relative == "" and child.name or relative .. "/" .. child.name
            if info.kind == "directory" then
                walk(path, rel)
            elseif info.kind == "file" then
                files[#files + 1] = { path = rel, kind = "file", size_bytes = info.size_bytes }
            else
                error("unsupported non-regular skill resource: " .. path)
            end
        end
        ancestors[actual] = nil
    end
    walk(root, "")
    table.sort(files, function(left, right) return left.path < right.path end)
    local found = false
    for _, file in ipairs(files) do if file.path == "SKILL.md" then found = true end end
    if not found then error("skill inventory must contain SKILL.md") end
    return files
end

local function register_skill(arguments, context)
    local path = normalized(arguments.path)
    if path:sub(1, 1) ~= "/" and not (WINDOWS and path:match("^%a:/")) then
        path = join(normalized(context.project_root), path)
    end
    local root, skill_md = root_for(path)
    local _, bytes = read_bytes(skill_md, root)
    local meta = metadata(bytes, root)
    local files = inventory(root)
    local paths = {}
    for _, file in ipairs(files) do paths[#paths + 1] = file.path end
    margatroid:tool_inject({ {
        alias = meta.name,
        description = meta.description,
        input_schema = {
            type = "object",
            properties = { path = {
                type = "string", enum = paths, default = "SKILL.md",
                description = "Read a registered skill file; omitted path reads SKILL.md. Scripts are returned as text and never executed.",
            } },
            additionalProperties = false,
        },
        binding = { kind = "skill", skill_md = skill_md, files = files },
    } })
    -- Static aliases survive this short task's finish without owner_task_id.
    return margatroid:json_encode({ alias = meta.name, skill_md = skill_md, files = files })
end

local function read_skill(arguments, context)
    local binding = context.binding
    if type(binding) ~= "table" or binding.kind ~= "skill" then error("invalid skill binding") end
    local root, actual = root_for(binding.skill_md)
    if parent(normalized(binding.skill_md)) ~= root or actual ~= normalized(binding.skill_md) then
        error("registered root has changed; register the skill again")
    end
    local files = binding.files
    if type(files) ~= "table" or #files > MAX_ENTRIES then error("registered inventory exceeds budget") end
    local path = arguments.path or "SKILL.md"
    if type(path) ~= "string" or path == "" or path:sub(1, 1) == "/"
        or (WINDOWS and (path:find("\\", 1, true) or path:find(":", 1, true))) then
        error("path must be a registered relative file path")
    end
    for part in path:gmatch("[^/]+") do
        if part == "." or part == ".." then error("path must be a registered relative file path") end
    end
    local found = false
    for _, file in ipairs(files) do if file.path == path then found = true end end
    if not found then error("path is not in the registered file inventory") end
    local target, bytes, size = read_bytes(join(root, path), root)
    local binary = bytes:find("\0", 1, true) ~= nil or utf8.len(bytes) == nil
    if path == "SKILL.md" then metadata(bytes, root) end
    if not binary then return bytes end
    return margatroid:json_encode({ path = target, relative_path = path,
        kind = "binary", size_bytes = size })
end

function execute(arguments, context)
    WINDOWS = package.config:sub(1, 1) == "\\"
    local ok, result = pcall(function()
        if context.binding == nil then return register_skill(arguments, context) end
        return read_skill(arguments, context)
    end)
    if ok then margatroid:task_return(result)
    else margatroid:task_return({ ok = false, content = "Skill: " .. tostring(result) }) end
    margatroid:task_finish(context.task_id)
end
