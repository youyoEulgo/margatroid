local MAX_MATCHES = 250
local MAX_LINE_BYTES = 2000

local function trim_newline(value)
    return value:gsub("[\r\n]+$", "")
end

function execute(arguments, context)
    local args = { "--json", "--no-config", "--color", "never" }
    if arguments.include then
        if arguments.include:sub(1, 1) == "!" or arguments.include:find(",", 1, true) then
            error("include must be one positive glob")
        end
        args[#args + 1] = "--glob"
        args[#args + 1] = arguments.include
    end
    args[#args + 1] = "--"
    args[#args + 1] = arguments.pattern
    args[#args + 1] = arguments.path or "."

    local result = margatroid.process.run({ program = "rg", args = args, cwd = context.project_root })
    if result.stdout_truncated then
        error("grep output exceeded the runtime limit; narrow the pattern or path")
    end
    if result.exit_code ~= 0 and result.exit_code ~= 1 then
        error("ripgrep failed: " .. trim_newline(result.stderr))
    end

    local matches = {}
    local total = 0
    for line in result.stdout:gmatch("[^\n]+") do
        local event = margatroid.json.decode(line)
        if event.type == "match" then
            total = total + 1
            if #matches < MAX_MATCHES then
                local path = event.data.path.text or "(unknown path)"
                local preview = trim_newline(event.data.lines.text or "")
                if #preview > MAX_LINE_BYTES then
                    preview = preview:sub(1, MAX_LINE_BYTES) .. " (line truncated)"
                end
                matches[#matches + 1] = string.format("%s:%d:%s", path, event.data.line_number, preview)
            end
        end
    end
    if total == 0 then return "No matches found." end
    if total > #matches then
        matches[#matches + 1] = string.format("[... %d additional matches omitted; narrow the search ...]", total - #matches)
    end
    return table.concat(matches, "\n")
end
