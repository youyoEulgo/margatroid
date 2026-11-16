function configure()
    return {timeout_ms = 0}
end

function execute(arguments, context)
    local agent = margatroid.agent:create({
        owner = context.workspace.address,
        name = context.name,
        config_path = context.resource_dir .. "/setting.toml",
        ui_path = context.resource_dir .. "/ui",
    })
    local active_model
    local turn
    local pending_tool_results = 0

    local function command(text, arguments)
        local result = margatroid.mcl:execute(agent.address, text, arguments)
        if result.Error then
            error(string.format("%s [MCL: %s; request_id: %s]", result.Error.message, text, result.Error.request_id), 2)
        end
        return result
    end

    local function settings()
        return command("CONFIG GET").Config
    end

    local function append_history(message)
        return command("EFFECT history_append ?", {{message}}).Array.Message[1]
    end

    local function enqueue_inference()
        local message = command("EFFECT inference (msg.instructions, session.conversation) ?", {turn}).Inference.message
        command("INJECT ? TO entry.pending[-1]", {{message}})
    end

    local function compaction_split(conversation, keep_messages)
        local count = 0
        for index = #conversation, 1, -1 do
            local kind = conversation[index].type
            if kind == "user" or kind == "assistant" then
                count = count + 1
                if count == keep_messages then return index end
            end
        end
        return 1
    end

    local function maybe_compact(message)
        local config = settings()
        local threshold = active_model.context_window_tokens * config.compaction.threshold
        if not message.usage then return false end
        if message.usage.input_tokens + message.usage.output_tokens < threshold then
            return false
        end
        local conversation = command("GET session.conversation").Array.Message
        return compaction_split(conversation, config.compaction.keep_messages) > 1
    end

    local function compact()
        local config = settings()
        local conversation = command("GET session.conversation").Array.Message
        local split = compaction_split(conversation, config.compaction.keep_messages)
        if split == 1 then
            return {type = "error", message = "No earlier context to compact"}
        end

        local earlier = {}
        for index = 1, split - 1 do
            earlier[#earlier + 1] = conversation[index]
        end
        command("INJECT ? TO msg.compact_context", {earlier})
        command("INJECT compact TO msg.compact_prompt")
        local response = command("EFFECT inference (msg.instructions, msg.compact_context, msg.compact_prompt)").Inference
        local message = response.message
        if message.type == "assistant" then message.type = "compact" end
        return message
    end

    local function process_compaction(message)
        message = append_history(message)
        if message.type == "error" then return end
        if message.type ~= "compact" or message.stop_reason ~= "completed"
            or not message.content or message.content == "" then return end

        local config = settings()
        local conversation = command("GET session.conversation").Array.Message
        local split = compaction_split(conversation, config.compaction.keep_messages)
        if split == 1 then return end
        local replacement = {
            {type = "user", content = "Conversation checkpoint:\n" .. message.content}
        }
        for index = split, #conversation do
            replacement[#replacement + 1] = conversation[index]
        end
        command("EFFECT block_snapshot ?", {"session"})
        command("INJECT ? TO session.conversation", {replacement})
        command("INJECT soul TO msg.instructions")
    end

    local function initialize()
        local config = settings()
        active_model = command("EFFECT model_set ?", {config.inference}).Config
        local initialization = config.initialization
        if not initialization then
            local initial = margatroid.data:decode("toml", margatroid.files:read(context.resource_dir .. "/setting.toml"))
            initialization = initial.initialization
        end
        for _, instruction in ipairs(initialization.default) do command(instruction) end
        for _, instruction in ipairs(initialization.custom or {}) do command(instruction) end
        command("EFFECT sandbox_use ?", {config.sandbox})
        command("CONFIG SET ? ?", {"driver_task", {agent = context.task_address, task_id = context.task_id}})
        command("EFFECT ready")
    end

    initialize()

    local function inject_context(message)
        for _, item in ipairs(message.messages or {}) do
            if item.type == "inject" then
                inject_context(item)
            else
                item = append_history(item)
                command("INJECT ? TO session.conversation[-1]", {{item}})
            end
        end
    end

    local function process_next()
        local message = command("GET entry.pending[0]").Array.Message[1]
        if not message then
            local source, value = margatroid:wait_any({
                wake = function() return margatroid:wait(agent.address) end,
                task = function() return margatroid.task:receive() end,
            })
            if source == "task" and type(value) == "table" and value.type == "compact" then
                process_compaction(compact())
            end
            return
        end
        command("INJECT [] TO entry.pending[0,1]")
        if message.type ~= "inject" then message = append_history(message) end

        if message.type == "inject" then
            inject_context(message)
        elseif message.type == "user" then
            command("INJECT ? TO session.conversation[-1]", {{message}})
            turn = command("EFFECT start_turn").Turn
            enqueue_inference()
        elseif message.type == "assistant" then
            if (message.content and message.content ~= "")
                or (message.reasoning and message.reasoning ~= "")
                or #(message.tool_calls or {}) > 0 then
                command("INJECT ? TO session.conversation[-1]", {{message}})
            end
            if #(message.tool_calls or {}) == 0 then
                if maybe_compact(message) then process_compaction(compact()) end
                command("EFFECT finish_turn")
            else
                local results = command("EFFECT tool_call (session.conversation[-1].tool_calls) ?", {turn}).Array.Message
                pending_tool_results = #results
                command("INJECT ? TO entry.pending[-1]", {results})
                if maybe_compact(message) then process_compaction(compact()) end
            end
        elseif message.type == "error" then
            pending_tool_results = 0
            command("EFFECT finish_turn")
        elseif message.type == "tool_call" then
            command("INJECT ? TO session.conversation[-1]", {{message}})
            pending_tool_results = pending_tool_results - 1
            if pending_tool_results == 0 then enqueue_inference() end
        end
    end

    while true do process_next() end
end
