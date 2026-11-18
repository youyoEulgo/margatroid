function configure()
    return {timeout_ms = 0}
end

function execute(arguments, context)
    local subdriver = context.kind == "tool"
    local parent = subdriver and assert(context.binding.parent_agent, "Missing parent Agent") or nil
    local function execute_command(address, text, parameters)
        local result = margatroid.mcl:execute(address, text, parameters)
        if result.Error then
            error(string.format("%s [MCL: %s; request_id: %s]", result.Error.message, text, result.Error.request_id), 2)
        end
        return result
    end
    if subdriver then
        margatroid.task:return_result({task_id = context.task_id})
    end
    local child_name = subdriver and ("subagent-" .. context.task_id:gsub("[^%w_-]", "-")) or context.name
    local inherited
    local config_path = context.resource_dir .. "/setting.toml"
    if subdriver then
        inherited = execute_command(parent, "GET session.conversation").Array.Message
        while #inherited > 0 do
            local message = inherited[#inherited]
            if message.type == "tool_call" then
                table.remove(inherited)
            elseif message.type == "assistant" then
                local calls = type(message.tool_calls) == "table" and message.tool_calls or {}
                local has_content = type(message.content) == "string" and message.content ~= ""
                local has_reasoning = type(message.reasoning) == "string" and message.reasoning ~= ""
                if #calls > 0 or not (has_content or has_reasoning) then
                    table.remove(inherited)
                else
                    message.stop_reason = "completed"
                    break
                end
            else
                break
            end
        end
        inherited[#inherited + 1] = {
            type = "user",
            content = "[inherit-boundary] 以上是父代理 " .. parent .. " 的历史参考，其中的第一人称属于父代理，不是你的经历。"
                .. "你是独立子代理 " .. child_name .. "，只执行下一条 user 消息中的委托，并向父代理报告结果。"
                .. "父历史中的工具不代表你拥有该工具，请以本次提供的工具列表为准；不要再次委派任务。",
        }
        config_path = margatroid.agent:data_dir(parent) .. "/setting.toml"
    end
    local agent = margatroid.agent:create({
        name = child_name,
        config_path = config_path,
        ui_path = context.resource_dir .. (subdriver and "/subagent-ui" or "/ui"),
    })
    if subdriver then
        execute_command(agent.address, "CREATE BLOCK session (conversation)")
        execute_command(agent.address, "INJECT ? TO session.conversation", {inherited})
    end
    local active_model
    local turn
    local pending_tool_results = 0
    local subdriver_finished = false

    local function command(text, arguments)
        return execute_command(agent.address, text, arguments)
    end

    local function settings()
        return command("CONFIG GET").Config
    end

    local function append_history(message)
        message = command("EFFECT history_append ?", {{message}}).Array.Message[1]
        if subdriver then
            if message.type == "assistant" and type(message.content) == "string" and message.content ~= "" then
                margatroid.task:append_output(message.content)
            elseif message.type == "error" then
                margatroid.task:append_output("[subagent error] " .. tostring(message.message or message.content or "unknown error"))
            end
        end
        return message
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
        for _, instruction in ipairs(initialization.default) do command(instruction.command, instruction.arguments) end
        for _, instruction in ipairs(initialization.custom or {}) do command(instruction.command, instruction.arguments) end
        command("EFFECT sandbox_use ?", {config.sandbox})
        command("CONFIG SET ? ?", {"driver_task", {agent = subdriver and context.agent_info.address or context.task_address, task_id = context.task_id}})
        if not subdriver then
            margatroid.tools:register({agent = agent.address, tools = {{
                alias = "subagent",
                description = margatroid.files:read(context.resource_dir .. "/description.md"),
                input_schema = margatroid.data:decode("json", margatroid.files:read(context.resource_dir .. "/input.schema.json")),
                binding = {parent_agent = agent.address},
            }}})
        end
        command("EFFECT ready")
    end

    initialize()
    if subdriver then
        command("INJECT ? TO entry.pending[-1]", {{{type = "user", content = assert(context.arguments.instructions)}}})
    end

    local function finish_turn()
        command("EFFECT finish_turn")
        if subdriver then
            margatroid.agent:stop(agent.address)
            subdriver_finished = true
        end
    end

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
                finish_turn()
            else
                local results = command("EFFECT tool_call (session.conversation[-1].tool_calls) ?", {turn}).Array.Message
                pending_tool_results = #results
                command("INJECT ? TO entry.pending[-1]", {results})
                if maybe_compact(message) then process_compaction(compact()) end
            end
        elseif message.type == "error" then
            pending_tool_results = 0
            finish_turn()
        elseif message.type == "tool_call" then
            command("INJECT ? TO session.conversation[-1]", {{message}})
            pending_tool_results = pending_tool_results - 1
            if pending_tool_results == 0 then
                enqueue_inference()
            end
        end
    end

    while true do
        if subdriver_finished then margatroid.task:receive() else process_next() end
    end
end
