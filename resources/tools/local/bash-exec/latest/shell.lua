-- Tool-private wire adapter; the host only supplies generic process/task IO.
return function(context)
    local client = {}
    local DEFAULT_OUTPUT_BYTES = 16 * 1024 * 1024
    local TERMINAL_HISTORY_BYTES = 128 * 1024 * 1024

    local function options_for(options, default)
        options = options or {}
        if options.timeout_ms ~= nil then error("timeout_ms belongs in configure(), not shell options") end
        local limit = options.max_output_bytes
        if limit == nil then limit = default end
        if type(limit) ~= "number" or limit < 0 or limit % 1 ~= 0 then
            error("max_output_bytes must be a nonnegative integer")
        end
        return limit
    end

    local function open(request, limit)
        local process = process_open({
            program = context.resource_dir .. "/bin/shell-client", arguments = {},
        })
        local state = {
            buffer = "", frames = {}, writes = { json.encode(request) .. "\n" },
            stdout_open = true, stderr_open = true, finished = false,
        }
        local session = {}
        -- stdout and stderr each have a payload budget; escaping can expand each byte sixfold.
        local frame_limit = limit * 12 + 8192

        local function consume(chunk)
            if type(chunk.data) ~= "string" then error("invalid shell-client stdout chunk") end
            state.buffer = state.buffer .. chunk.data
            while true do
                local newline = state.buffer:find("\n", 1, true)
                if not newline then break end
                if newline > frame_limit then error("shell-client frame exceeds byte budget") end
                local line = state.buffer:sub(1, newline - 1)
                state.buffer = state.buffer:sub(newline + 1)
                local frame = json.decode(line)
                if type(frame) ~= "table" or type(frame.type) ~= "string" then
                    error("invalid shell-client frame")
                end
                state.frames[#state.frames + 1] = frame
            end
            if #state.buffer > frame_limit then error("shell-client frame exceeds byte budget") end
            if chunk.eof then
                state.stdout_open = false
                if state.buffer ~= "" then error("shell-client EOF inside JSONL frame") end
            end
        end

        local function pump(writing)
            local operations = {}
            if state.stdout_open then operations.stdout = function() return process:read() end end
            if state.stderr_open then operations.stderr = function() return process:read_stderr() end end
            if writing and #state.writes > 0 then
                operations.write = function()
                    -- Cancellation retains process write progress. Retry this exact
                    -- frame, and remove it only once the entire write completes.
                    process:write(state.writes[1])
                    table.remove(state.writes, 1)
                end
            end
            if next(operations) == nil then return false end
            local source, chunk = await_any(operations)
            if source == "stdout" then consume(chunk)
            elseif source == "stderr" then
                -- Diagnostics are private, but drain them independently to EOF.
                state.stderr_open = not chunk.eof
            end
            return true
        end

        function session:read()
            while #state.frames == 0 do
                if state.finished or not pump(true) then return nil end
            end
            local frame = table.remove(state.frames, 1)
            if frame.type == "error" then error(frame.message or "shell-client error") end
            return frame
        end

        function session:finish()
            if not state.finished then
                -- Never race wait against a read: a ready wait can hide tail data.
                while state.stdout_open or state.stderr_open do pump(false) end
                state.status = process:wait()
                state.finished = true
            end
            return state.status
        end

        function session:write(text)
            if type(text) ~= "string" then error("terminal input must be a string") end
            state.writes[#state.writes + 1] = json.encode({ operation = "write", text = text }) .. "\n"
        end

        function session:close()
            if state.finished then return end
            if not state.close_requested then
                state.writes[#state.writes + 1] = json.encode({ operation = "close" }) .. "\n"
                state.close_requested = true
            end
            while #state.writes > 0 do pump(true) end
            self:finish()
        end

        return session
    end

    function client.exec(command, options)
        if type(command) ~= "string" then error("command must be a string") end
        local limit = options_for(options, DEFAULT_OUTPUT_BYTES)
        if options and options.pty ~= nil and type(options.pty) ~= "boolean" then
            error("pty must be a boolean")
        end
        local session = open({ operation = "exec", command = command,
            max_output_bytes = limit, pty = options and options.pty or false }, limit)
        local result = session:read()
        if not result or result.type ~= "result" then error("shell-client missing exec result") end
        -- Consume the result before waiting; finish drains both pipes first.
        session:finish()
        if session:read() ~= nil then error("unexpected shell-client exec frame") end
        return result
    end

    function client.terminal(options)
        local limit = options_for(options, TERMINAL_HISTORY_BYTES)
        local session = open({ operation = "terminal", max_output_bytes = limit }, limit)
        local ready = session:read()
        if not ready or ready.type ~= "ready" then error("shell-client missing terminal ready") end
        local terminal = {}
        local final_exit
        function terminal:write(text) session:write(text) end
        function terminal:close() session:close() end
        function terminal:read()
            local frame = final_exit or session:read()
            if not frame then error("shell-client missing terminal exit") end
            if frame.type == "output" and type(frame.text) == "string" and type(frame.replace) == "boolean" then
                return { text = frame.text, replace = frame.replace }
            end
            if frame.type == "exit" and frame.eof == true then
                final_exit = frame
                session:finish()
                if session:read() ~= nil then error("unexpected frame after terminal exit") end
                return { eof = true, exit_code = frame.exit_code }
            end
            error("unexpected shell-client terminal frame")
        end
        return terminal
    end

    return client
end
