-- Author budgets, independent of the host's stricter execution limits.
local FRAME_BYTES = 128 * 1024 * 1024
local BUFFER_BYTES = FRAME_BYTES + 8192
local MAX_PENDING = 4096
local MAX_QUEUE = 4096
local QUEUE_BYTES = 2 * FRAME_BYTES

function configure(arguments, context)
    if context.owner_task_id == nil then return { timeout_ms = 0 } end
    return { timeout_ms = 30000 }
end

local function failure(message)
    return { ok = false, content = "MCP: " .. tostring(message) }
end

local function send_reply(task_id, response)
    local ok, err = pcall(task_send, task_id, response)
    -- Only an already removed B may be forgotten; all other errors are fatal.
    if not ok and not tostring(err):find("TaskMissing:", 1, true) then error(err) end
end

local function rpc_error(code, message)
    return { code = code, message = tostring(message) }
end

local function object(value)
    -- json.decode preserves the JSON object/array distinction when re-encoded.
    return type(value) == "table" and json.encode(value):sub(1, 1) == "{"
end

local function format_result(response)
    if response.error ~= nil then
        return failure(json.encode(response.error))
    end
    local result = response.result
    if not object(result) or type(result.content) ~= "table" then
        return failure("invalid MCP call result: expected an object with content array")
    end
    local text, images = {}, {}
    for _, item in ipairs(result.content) do
        if type(item) ~= "table" then error("invalid MCP content item") end
        if item.type == "text" and type(item.text) == "string" then
            text[#text + 1] = item.text
        elseif item.type == "image" and type(item.data) == "string"
            and type(item.mimeType) == "string" and result.isError ~= true then
            images[#images + 1] = { media_type = item.mimeType, data = item.data }
        else
            -- Preserve resource/audio/unknown content, and images on failed calls,
            -- in text: the host rejects images on failed result envelopes.
            text[#text + 1] = json.encode(item)
        end
    end
    if result.structuredContent ~= nil then
        text[#text + 1] = json.encode(result.structuredContent)
    end
    local envelope = { ok = result.isError ~= true, content = table.concat(text, "\n") }
    if #images > 0 then envelope.images = images end
    return envelope
end

local function call_remote(arguments, context)
    if type(context.binding) ~= "table" or type(context.binding.remote_tool) ~= "string" then
        error("missing remote_tool binding")
    end
    task_send(context.owner_task_id, {
        operation = "call", id = context.task_id, reply_to = context.task_id,
        tool = context.binding.remote_tool, arguments = arguments,
    })
    while true do
        local response = task_receive()
        if type(response) == "table" and response.id == context.task_id then
            task_return(format_result(response))
            return
        end
    end
end

local function register_server(arguments, context, state)
    local process = process_open({
        program = context.resource_dir .. "/bin/mcp-client", arguments = {},
    })
    state.process = process
    local queue, queue_bytes = {}, 0
    local pending, pending_count = state.pending, 0
    local buffer, stderr_open = "", true
    local register_id = context.task_id
    local discovery = nil
    local registered = false
    local register_frame
    -- Empty Lua tables serialize as arrays. Construct the exact wire with JSON
    -- defaults explicitly, so args is [] while env/headers remain {}.
    local server = arguments.server
    if server.transport == "stdio" then
        register_frame = '{"id":' .. json.encode(register_id)
            .. ',"operation":"register","server":{"transport":"stdio","command":'
            .. json.encode(server.command) .. ',"args":'
            .. (server.args and json.encode(server.args) or "[]") .. ',"env":'
            .. (server.env and json.encode(server.env) or "{}") .. '}}\n'
    else
        register_frame = '{"id":' .. json.encode(register_id)
            .. ',"operation":"register","server":{"transport":"http","url":'
            .. json.encode(server.url) .. ',"headers":'
            .. (server.headers and json.encode(server.headers) or "{}") .. '}}\n'
    end

    local function enqueue(frame)
        if #frame > FRAME_BYTES then return false, "request frame exceeds byte budget" end
        if #queue >= MAX_QUEUE or queue_bytes + #frame > QUEUE_BYTES then
            return false, "request write queue exceeds budget"
        end
        queue[#queue + 1] = { frame = frame }
        queue_bytes = queue_bytes + #frame
        return true
    end
    local queued, queue_error = enqueue(register_frame)
    if not queued then error(queue_error) end

    local function write_next()
        local entry = queue[1]
        if not entry then return end
        -- The process API retains a partially written frame across await_any
        -- cancellation. Keep the same queue head until that write completes.
        process:write(entry.frame)
        queue_bytes = queue_bytes - #entry.frame
        table.remove(queue, 1)
    end

    local function handle_response(response)
        if not object(response) or type(response.id) ~= "string"
            or ((response.result ~= nil) == (response.error ~= nil)) then
            error("invalid response frame: expected id and exactly one of result/error")
        end
        if response.error ~= nil and (not object(response.error)
            or type(response.error.message) ~= "string" or response.error.code == nil) then
            error("invalid response error frame")
        end
        if not registered and response.id == register_id then
            if discovery ~= nil then error("duplicate registration response") end
            discovery = response
        elseif pending[response.id] ~= nil then
            local reply_to = pending[response.id]
            pending[response.id] = nil
            pending_count = pending_count - 1
            send_reply(reply_to, response)
        else
            task_write({ diagnostic = "ignored unknown or duplicate MCP response id", id = response.id })
        end
    end

    local function consume_stdout(chunk)
        if type(chunk.data) ~= "string" then error("invalid raw stdout read") end
        if #buffer + #chunk.data > BUFFER_BYTES then error("stdout buffer exceeds byte budget") end
        buffer = buffer .. chunk.data
        while true do
            local newline = buffer:find("\n", 1, true)
            if not newline then break end
            if newline > FRAME_BYTES then error("response frame exceeds byte budget") end
            local line = buffer:sub(1, newline - 1)
            buffer = buffer:sub(newline + 1)
            if line == "" then error("empty response frame") end
            local ok, response = pcall(json.decode, line)
            if not ok then error("invalid JSON response frame: " .. tostring(response)) end
            handle_response(response)
        end
        if #buffer >= FRAME_BYTES then error("response frame exceeds byte budget") end
        if chunk.eof then
            if #buffer > 0 then error("MCP client EOF with incomplete response frame") end
            error("MCP client stdout closed")
        end
    end

    local function inject_discovery()
        if discovery.error ~= nil then error(json.encode(discovery.error)) end
        local result = discovery.result
        if not object(result) or type(result.tools) ~= "table"
            or json.encode(result.tools):sub(1, 1) ~= "[" then
            error("invalid discovery result: expected tools array")
        end
        local prefix = arguments.alias_prefix or "mcp_"
        local definitions, aliases, seen = {}, {}, {}
        for _, tool in ipairs(result.tools) do
            if not object(tool) or type(tool.name) ~= "string" or tool.name == ""
                or not object(tool.inputSchema)
                or (tool.description ~= nil and type(tool.description) ~= "string") then
                error("invalid discovered tool definition")
            end
            local alias = prefix .. tool.name:gsub("[^A-Za-z0-9_-]", "_")
            if seen[alias] then error("discovered tool aliases collide: " .. alias) end
            seen[alias] = true
            aliases[#aliases + 1] = alias
            local description = tool.description
            if description == nil or description:match("^%s*$") then
                description = tool.name
            end
            definitions[#definitions + 1] = {
                alias = alias, description = description,
                input_schema = tool.inputSchema, binding = { remote_tool = tool.name },
            }
        end
        -- The host validates and commits the entire batch to its existing Tools.
        tool_inject(definitions)
        registered = true
        -- Encode explicitly so an empty aliases list remains [] in the result.
        task_return('{"task_id":' .. json.encode(context.task_id) .. ',"aliases":'
            .. (#aliases == 0 and "[]" or json.encode(aliases)) .. '}')
        state.returned = true
    end

    while true do
        local operations = {
            stdout = function() return process:read() end,
            exited = function() return process:wait() end,
        }
        if stderr_open then operations.stderr = function() return process:read_stderr() end end
        if #queue > 0 then operations.write = write_next end
        if registered then operations.input = function() return task_receive() end end
        local source, value = await_any(operations)
        if source == "stdout" then
            consume_stdout(value)
        elseif source == "stderr" then
            -- Diagnostics are intentionally discarded, but always drained.
            stderr_open = not value.eof
        elseif source == "exited" then
            error("MCP client exited: " .. json.encode(value))
        elseif source == "input" then
            if type(value) ~= "table" or value.operation ~= "call"
                or type(value.id) ~= "string" or type(value.reply_to) ~= "string"
                or type(value.tool) ~= "string" or not object(value.arguments) then
                error("invalid owner call message")
            end
            if pending[value.id] then error("duplicate pending call id") end
            local frame = json.encode({ id = value.id, operation = "call",
                tool = value.tool, arguments = value.arguments }) .. "\n"
            local accepted, reason = false, "in-flight requests exceed budget"
            if pending_count < MAX_PENDING then accepted, reason = enqueue(frame) end
            if accepted then
                pending[value.id] = value.reply_to
                pending_count = pending_count + 1
            else
                send_reply(value.reply_to, { id = value.id,
                    error = rpc_error("budget_exceeded", reason) })
            end
        end
        if discovery and not registered then inject_discovery() end
    end
end

function execute(arguments, context)
    local state = { pending = {}, returned = false }
    local ok, err = pcall(function()
        if context.owner_task_id == nil then
            register_server(arguments, context, state)
        else
            call_remote(arguments, context)
            state.returned = true
        end
    end)
    if not ok then
        -- Keep draining associations even if one reply fails. Only TaskMissing
        -- is swallowed by send_reply; other delivery failures are diagnosed.
        for id, reply_to in pairs(state.pending) do
            local sent, send_error = pcall(send_reply, reply_to,
                { id = id, error = rpc_error("disconnected", err) })
            state.pending[id] = nil
            if not sent then task_write({ error = tostring(send_error) }) end
        end
        if not state.returned then task_return(failure(err))
        else task_write({ error = "MCP session ended: " .. tostring(err) }) end
    end
    -- Self finish never returns. Keep it outside pcall; host worker cleanup owns
    -- process termination and releases all dynamic aliases and dependent tasks.
    task_finish(context.task_id)
end
