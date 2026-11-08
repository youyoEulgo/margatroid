function configure(arguments, context)
    return { timeout_ms = 30000 }
end

local function expected(message)
    error({ file_failure = true, message = message }, 0)
end
local function display(bytes, label)
    if utf8.len(bytes) then return bytes end
    return "[base64 " .. label .. ": " .. margatroid.data:encode("base64", bytes) .. "]"
end
-- Match rg's automatic BOM detection; byte strings without a BOM remain raw.
local function decoded(bytes)
    if bytes:sub(1, 3) == "\239\187\191" then return bytes:sub(4) end
    local bom = bytes:sub(1, 2)
    if bom ~= "\255\254" and bom ~= "\254\255" then return bytes end
    local little = bom == "\255\254"
    local function unit(index)
        local a, b = bytes:byte(index, index + 1)
        if not b then return nil end
        return little and a + b * 256 or a * 256 + b
    end
    local output, index = {}, 3
    while index <= #bytes do
        local code = unit(index)
        index = index + 2
        if not code then code = 65533
        elseif code >= 55296 and code <= 56319 then
            local low = unit(index)
            if low and low >= 56320 and low <= 57343 then
                code, index = 65536 + (code - 55296) * 1024 + low - 56320, index + 2
            else code = 65533 end
        elseif code >= 56320 and code <= 57343 then code = 65533 end
        output[#output + 1] = utf8.char(code)
    end
    return table.concat(output)
end

local function perform(arguments, context)
    if arguments.include and arguments.include:sub(1, 1) == "!" then
        return { ok = false, content = "include must be one positive glob" }
    end
    if arguments.include then margatroid.files:matches("", arguments.include) end
    margatroid.text:search(arguments.pattern, "")
    local helper = assert(load(margatroid.files:read(context.resource_dir .. "/filewalk.lua"), "@filewalk.lua"))()
    local matches, total, output_bytes = {}, 0, 0
    helper.walk(arguments.path or ".", { ignore = true, include = arguments.include }, function(info, path, explicit)
        if not explicit and arguments.include and not margatroid.files:matches(path:gsub("^%./", ""), arguments.include) then return end
        local content = decoded(margatroid.files:read(info.path))
        local start, number = 1, 0
        while start <= #content do
            local newline = content:find("\n", start, true)
            local line = content:sub(start, newline and newline - 1 or #content)
            number = number + 1
            local found = margatroid.text:search(arguments.pattern, line)
            if #found > 0 then
                total = total + 1
                local raw_line = content:sub(start, newline or #content)
                local preview = display(raw_line, "bytes"):gsub("[\r\n]+$", "")
                local shown = display(path, "path"):gsub("\r", "\\r"):gsub("\n", "\\n")
                local function wire(value)
                    if utf8.len(value) then return { text = value } end
                    return { bytes = margatroid.data:encode("base64", value) }
                end
                local submatches = {}
                for _, match in ipairs(found) do
                    submatches[#submatches + 1] = { match = wire(line:sub(match.start + 1, match["end"])), start = match.start, ["end"] = match["end"] }
                end
                output_bytes = output_bytes + #margatroid.data:encode("json", { type = "match", data = {
                    path = wire(path), lines = wire(raw_line), line_number = number,
                    absolute_offset = start - 1, submatches = submatches } }) + 1
                if output_bytes > 16 * 1024 * 1024 then expected("grep output exceeded the size limit; narrow the pattern or path") end
                if #matches < 250 then
                    if #preview > 2000 then
                        local cut = 2000
                        while cut > 0 and not utf8.len(preview:sub(1, cut)) do cut = cut - 1 end
                        preview = preview:sub(1, cut) .. " (line truncated)"
                    end
                    matches[#matches + 1] = string.format("%s:%d:%s", shown, number, preview)
                end
            end
            if not newline then break end
            start = newline + 1
        end
    end)
    if total == 0 then return { ok = true, content = "No matches found." } end
    if total > #matches then matches[#matches + 1] = string.format("[... %d additional matches omitted; narrow the search ...]", total - #matches) end
    return { ok = true, content = table.concat(matches, "\n") }
end

function execute(arguments, context)
    arguments = context.arguments
    local ok, result = pcall(perform, arguments, context)
    if ok then margatroid.task:return_result(result)
    elseif type(result) == "table" and result.file_failure then
        margatroid.task:return_result({ ok = false, content = result.message })
    else error(result, 0) end
    margatroid.task:finish(context.task_id)
end
