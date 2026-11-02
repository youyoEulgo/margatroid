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

-- The host shell owns timeout and output budgets.
local function run(command)
    local result = margatroid().shell(command, { max_output_bytes = MAX_OUTPUT_BYTES })
    if result.truncated then
        return nil, "grep output exceeded the size limit; narrow the pattern or path"
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
    local args = { "--json", "--no-config", "--color", "never" }
    if arguments.include then
        if arguments.include:sub(1, 1) == "!" then
            return report(nil, "include must be one positive glob")
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
    local stdout, failure = run(table.concat(parts, " "))
    if not stdout then return report(nil, failure) end

    local json = margatroid().json
    local matches = {}
    local total = 0
    for line in stdout:gmatch("[^\n]+") do
        local event = json.decode(line)
        if event.type == "match" then
            total = total + 1
            if #matches < MAX_MATCHES then
                local path = event.data.path.text or ("[base64 path: " .. tostring(event.data.path.bytes) .. "]")
                local preview = trim_newline(event.data.lines.text or ("[base64 bytes: " .. tostring(event.data.lines.bytes) .. "]"))
                if #preview > MAX_LINE_BYTES then
                    local cut = MAX_LINE_BYTES
                    while cut > 0 and not utf8.len(preview:sub(1, cut)) do cut = cut - 1 end
                    preview = preview:sub(1, cut) .. " (line truncated)"
                end
                matches[#matches + 1] = string.format("%s:%d:%s", path:gsub("\r", "\\r"):gsub("\n", "\\n"), event.data.line_number, preview)
            end
        end
    end
    if total == 0 then return report("No matches found.") end
    if total > #matches then
        matches[#matches + 1] = string.format("[... %d additional matches omitted; narrow the search ...]", total - #matches)
    end
    return report(table.concat(matches, "\n"))
end

function execute(arguments, context)
    local result = perform(arguments, context)
    task_return(result)
    task_finish(context.task_id)
end
