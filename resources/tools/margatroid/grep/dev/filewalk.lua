-- Shared source, packaged in glob and grep; all state belongs to one execute.
local M = {}
local files = margatroid.files
local vcs = { [".git"] = true, [".hg"] = true, [".svn"] = true, [".jj"] = true }
local function join(a, b) return a:gsub("/$", "") .. "/" .. b end
local function parent(path) return path:match("^(.*)/[^/]+/*$") or "." end
local function optional_info(path)
    local ok, value = pcall(function() return files:info(path) end)
    if ok then return value end
    local message = type(value) == "table" and value.file_failure and value.message or tostring(value)
    if message:find("No such file", 1, true) or message:find("NotFound", 1, true)
        or message:find("not found", 1, true) then return nil end
    error(value, 0)
end
local function add_rules(rules, directory, filename, priority, source)
    local info = optional_info(source or join(directory, filename))
    if not info or info.kind == "directory" or not info.resolved_path then return end
    local bytes = files:read(info.path)
    for raw in (bytes .. "\n"):gmatch("([^\n]*)\n") do
        local line = raw:gsub("\r$", "")
        while line:sub(-1) == " " do
            local slash = 0
            for i = #line - 1, 1, -1 do if line:sub(i, i) ~= "\\" then break end; slash = slash + 1 end
            if slash % 2 == 1 then line = line:sub(1, -3) .. " "; break end
            line = line:sub(1, -2)
        end
        if line ~= "" and line:sub(1, 1) ~= "#" then
            local negate = line:sub(1, 1) == "!"
            if negate then line = line:sub(2) end
            local dironly = line:sub(-1) == "/"
            if dironly then line = line:sub(1, -2) end
            local anchored = line:sub(1, 1) == "/"
            if anchored then line = line:sub(2) end
            if line ~= "" then
                local valid = pcall(function() files:matches("", line) end)
                if valid then rules[#rules + 1] = { base = directory, pattern = line,
                    negate = negate, dironly = dironly, anchored = anchored, priority = priority } end
            end
        end
    end
end
local function global_ignore()
    local home = os.getenv("HOME")
    local xdg = os.getenv("XDG_CONFIG_HOME")
    if not xdg or xdg == "" then xdg = home and join(home, ".config") end
    local configured
    -- rg reads direct core.excludesFile values without expanding Git includes.
    local function config(path)
        if not path then return end
        local info = optional_info(path)
        if not info or not info.resolved_path or info.kind == "directory" then return end
        local section
        for raw in (files:read(path) .. "\n"):gmatch("([^\n]*)\n") do
            local line = raw:gsub("^%s+", ""):gsub("%s+$", "")
            local header = line:match("^%[([^%]]+)%]")
            if header then section = header:lower()
            elseif line:sub(1, 1) ~= "#" and line:sub(1, 1) ~= ";" then
                local key, value = line:match("^([%w]+)%s*=%s*(.-)%s*$")
                if key then
                    if value:sub(1, 1) == '"' and value:sub(-1) == '"' then
                        value = value:sub(2, -2):gsub('\\(.)', function(c)
                            return ({ n = "\n", t = "\t", b = "\b", ['"'] = '"', ['\\'] = '\\' })[c] or c
                        end)
                    else value = value:gsub("%s+[;#].*$", "") end
                    if value:sub(1, 2) == "~/" and home then value = join(home, value:sub(3)) end
                    if section == "core" and key:lower() == "excludesfile" then configured = value end
                end
            end
        end
    end
    local explicit = os.getenv("GIT_CONFIG_GLOBAL")
    if explicit then config(explicit)
    else
        if xdg then config(join(xdg, "git/config")) end
        if home then config(join(home, ".gitconfig")) end
    end
    local path = configured or (xdg and join(xdg, "git/ignore"))
    if path and path ~= "" then
        -- Global rules are relative to the search root; caller assigns their base.
        return path
    end
end

local function ignored(rules, path, directory)
    local result, priority = false, 0
    for _, rule in ipairs(rules) do
        if rule.priority >= priority and path:sub(1, #rule.base + 1) == rule.base .. "/" then
            local relative = path:sub(#rule.base + 2)
            if (not rule.dironly or directory) and (not rule.anchored or relative:find("/", 1, true) == nil or rule.pattern:find("/", 1, true))
                and files:matches(relative, rule.pattern) then
                result, priority = not rule.negate, rule.priority
            end
        end
    end
    return result
end
function M.walk(path, options, visit)
    local initial = files:info(path)
    local actual = initial.resolved_path
    if initial.kind == "symlink" and not actual then error({ file_failure = true, message = "cannot resolve " .. path }, 0) end
    local target = initial.kind == "symlink" and actual and files:info(actual) or initial
    if not target then error("cannot resolve " .. path, 0) end
    if target.kind ~= "directory" then
        if target.kind == "file" then visit(target, path, true) end
        return
    end
    local absolute = initial.resolved_path or initial.path
    local rules, git_root = {}, nil
    if options.ignore then
        local chain, cursor = {}, absolute
        while true do
            table.insert(chain, 1, cursor)
            local up = parent(cursor)
            if up == "" then up = "/" end
            if up == cursor or cursor == "/" or cursor == "." then break end
            cursor = up
        end
        for i = #chain, 1, -1 do
            if optional_info(join(chain[i], ".git")) then git_root = chain[i]; break end
        end
        if git_root then
            local global = global_ignore()
            if global then add_rules(rules, git_root, "", 1, global) end
            add_rules(rules, git_root, ".git/info/exclude", 1)
        end
        for _, directory in ipairs(chain) do
            if directory ~= absolute then
                if git_root and (directory == git_root or directory:sub(1, #git_root + 1) == git_root .. "/") then add_rules(rules, directory, ".gitignore", 2) end
                add_rules(rules, directory, ".ignore", 3)
                add_rules(rules, directory, ".rgignore", 4)
            end
        end
    end
    local function walk(directory, display)
        local count = #rules
        if options.ignore then
            if git_root then add_rules(rules, directory, ".gitignore", 2) end
            add_rules(rules, directory, ".ignore", 3)
            add_rules(rules, directory, ".rgignore", 4)
        end
        for _, entry in ipairs(files:list(directory)) do
            local directory_entry = entry.kind == "directory"
            local shown = join(display, entry.name)
            local skip = directory_entry and vcs[entry.name]
            if directory_entry and options.exclude then
                skip = skip or files:matches(shown:gsub("^%./", ""), options.exclude:gsub("/$", ""))
            end
            if options.ignore then
                local selected = options.include and not directory_entry and files:matches(shown:gsub("^%./", ""), options.include)
                skip = skip or (not selected and (entry.name:sub(1, 1) == "." or ignored(rules, entry.path, directory_entry)))
            end
            if not skip then
                if directory_entry then walk(entry.path, shown)
                elseif entry.kind == "file" then visit(entry, shown) end
            end
        end
        while #rules > count do rules[#rules] = nil end
    end
    walk(absolute, path)
end
return M
