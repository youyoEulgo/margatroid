local MAX_MATCHES = 250
local MAX_LINE_BYTES = 2000
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
        return nil, "grep output exceeded the size limit; narrow the pattern or path"
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

    local parts = { "rg" }
    for _, arg in ipairs(args) do
        parts[#parts + 1] = shell_quote(arg)
    end
    local stdout, code = run(table.concat(parts, " "))
    if code ~= 0 and code ~= 1 then
        return report(nil, "ripgrep failed")
    end

    local json = margatroid().json
    local matches = {}
    local total = 0
    for line in stdout:gmatch("[^\n]+") do
        local event = json.decode(line)
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
    if total == 0 then return report("No matches found.") end
    if total > #matches then
        matches[#matches + 1] = string.format("[... %d additional matches omitted; narrow the search ...]", total - #matches)
    end
    return report(table.concat(matches, "\n"))
end
