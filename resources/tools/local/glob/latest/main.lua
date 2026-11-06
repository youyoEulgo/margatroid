function configure(arguments, context)
    return { timeout_ms = 30000 }
end

local MAX_RESULTS = 100
local MAX_OUTPUT_BYTES = 16 * 1024 * 1024

local function shell_quote(value)
    if package.config:sub(1, 1) == "\\" then
        return '"' .. value:gsub('"', '\\"') .. '"'
    end
    return "'" .. value:gsub("'", "'\\''") .. "'"
end

-- Task configuration owns timeout; the private client bounds command output.
local function run(command, context)
    local client = dofile(context.resource_dir .. "/shell.lua")(context)
    local result = client.exec(command, { max_output_bytes = MAX_OUTPUT_BYTES })
    if result.truncated then
        return nil, "glob output exceeded the size limit; narrow the pattern or path"
    end
    if result.exit_code ~= 0 and result.exit_code ~= 1 then
        return nil, "ripgrep failed (exit " .. tostring(result.exit_code) .. "): " .. result.stderr
    end
    return result.stdout
end

local function report(content, failure)
    if failure then
        return { ok = false, content = failure }
    end
    return { ok = true, content = content }
end

local function perform(arguments, context)
    local args = {
        "--files", "--null", "--no-config", "--hidden", "--no-ignore", "--sortr", "modified",
        "--glob", arguments.pattern,
        "--glob", "!**/.git/**", "--glob", "!**/.hg/**", "--glob", "!**/.svn/**",
        "--glob", "!**/.jj/**", "--", arguments.path or ".",
    }
    local parts = { "rg" }
    for _, arg in ipairs(args) do
        parts[#parts + 1] = shell_quote(arg)
    end
    local stdout, failure = run(table.concat(parts, " "), context)
    if not stdout then return report(nil, failure) end

    local paths = {}
    local total = 0
    for path in stdout:gmatch("([^%z]+)%z") do
        total = total + 1
        if #paths < MAX_RESULTS then paths[#paths + 1] = path:gsub("\r", "\\r"):gsub("\n", "\\n") end
    end
    if total == 0 then return report("No files found.") end
    if total > #paths then
        paths[#paths + 1] = string.format("[... %d additional files omitted; narrow the search ...]", total - #paths)
    end
    return report(table.concat(paths, "\n"))
end

function execute(arguments, context)
    local result = perform(arguments, context)
    task_return(result)
    task_finish(context.task_id)
end
