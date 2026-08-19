local MAX_RESULTS = 100

local function trim_newline(value)
    return value:gsub("[\r\n]+$", "")
end

function execute(arguments, context)
    local args = {
        "--files", "--no-config", "--hidden", "--no-ignore", "--sort", "modified",
        "--glob", arguments.pattern,
        "--glob", "!.git/**", "--glob", "!.hg/**", "--glob", "!.svn/**",
        "--glob", "!.jj/**", "--", arguments.path or ".",
    }
    local result = margatroid.process.run({ program = "rg", args = args, cwd = context.project_root })
    if result.stdout_truncated then
        error("glob output exceeded the runtime limit; narrow the pattern or path")
    end
    if result.exit_code ~= 0 and result.exit_code ~= 1 then
        error("ripgrep failed: " .. trim_newline(result.stderr))
    end

    local paths = {}
    local total = 0
    for path in result.stdout:gmatch("[^\r\n]+") do
        total = total + 1
        if #paths < MAX_RESULTS then paths[#paths + 1] = path end
    end
    if total == 0 then return "No files found." end
    if total > #paths then
        paths[#paths + 1] = string.format("[... %d additional files omitted; narrow the search ...]", total - #paths)
    end
    return table.concat(paths, "\n")
end
