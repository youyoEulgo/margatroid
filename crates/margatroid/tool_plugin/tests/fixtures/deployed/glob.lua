local MAX_RESULTS = 100
local MAX_OUTPUT_BYTES = 16 * 1024 * 1024

local function trim_newline(value)
    return value:gsub("[\r\n]+$", "")
end

local function shell_quote(value)
    if package.config:sub(1, 1) == "\\" then
        return '"' .. value:gsub('"', '\\"') .. '"'
    end
    return "'" .. value:gsub("'", "'\\''") .. "'"
end

-- The command runs through the shell the runner provides, which gives it a
-- terminal, a process group that is reclaimed on timeout, and an execution
-- budget. io.popen would give none of those.
local function run(command)
    local result = margatroid().shell(command, { max_output_bytes = MAX_OUTPUT_BYTES })
    if result.truncated then
        return nil, "glob output exceeded the size limit; narrow the pattern or path"
    end
    return result.stdout, result.exit_code
end

-- One envelope for every outcome: `content` is what the model reads, `ok` is
-- what tells the runtime and the interface whether the call failed.
local function report(content, failure)
    if failure then
        return margatroid().json.encode({ ok = false, content = failure })
    end
    return margatroid().json.encode({ ok = true, content = content })
end

function execute(arguments, context)
    local args = {
        "--files", "--no-config", "--hidden", "--no-ignore", "--sort", "modified",
        "--glob", arguments.pattern,
        "--glob", "!.git/**", "--glob", "!.hg/**", "--glob", "!.svn/**",
        "--glob", "!.jj/**", "--", arguments.path or ".",
    }
    local parts = { "rg" }
    for _, arg in ipairs(args) do
        parts[#parts + 1] = shell_quote(arg)
    end
    local stdout, code = run(table.concat(parts, " "))
    if code ~= 0 and code ~= 1 then
        return report(nil, "ripgrep failed")
    end

    local paths = {}
    local total = 0
    for path in stdout:gmatch("[^\r\n]+") do
        total = total + 1
        if #paths < MAX_RESULTS then paths[#paths + 1] = path end
    end
    if total == 0 then return report("No files found.") end
    if total > #paths then
        paths[#paths + 1] = string.format("[... %d additional files omitted; narrow the search ...]", total - #paths)
    end
    return report(table.concat(paths, "\n"))
end
