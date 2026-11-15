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
        local message = command("EFFECT inference (msg.instructions, session.conversation)").Inference.message
        command("INJECT ? TO entry.pending[-1]", {{message}})
    end

    local function maybe_compact_context(message)
        local config = settings()
        local threshold = active_model.context_window_tokens * config.compaction.threshold
        if not message.usage
            or message.usage.input_tokens + message.usage.output_tokens < threshold then
            return
        end
        local conversation = command("GET session.conversation").Array.Message
        local count = 0
        local split = 1
        for index = #conversation, 1, -1 do
            local kind = conversation[index].type
            if kind == "user" or kind == "assistant" then
                count = count + 1
                if count == config.compaction.keep_messages then
                    split = index
                    break
                end
            end
        end
        if split == 1 then return end

        local earlier = {}
        local recent = {}
        for index, item in ipairs(conversation) do
            if index < split then
                earlier[#earlier + 1] = item
            else
                recent[#recent + 1] = item
            end
        end
        command("INJECT ? TO msg.compact_context", {earlier})
        local summary = command("EFFECT inference (msg.instructions, msg.compact_context, msg.compact_prompt)").Inference
        summary.message = append_history(summary.message)
        if summary.stop_reason == "completed" and summary.message.type == "assistant"
            and summary.message.content and summary.message.content ~= "" then
            local replacement = {
                {type = "user", content = "Conversation checkpoint:\n" .. summary.message.content}
            }
            for _, item in ipairs(recent) do
                replacement[#replacement + 1] = item
            end
            command("EFFECT block_snapshot ?", {"session"})
            command("INJECT ? TO session.conversation", {replacement})
        end
    end

    local function initialize()
        local config = settings()
        active_model = command("EFFECT model_set ?", {config.inference}).Config
        command("IMPORT TOOL tool:local/skill-register:latest AS skill_register")
        command("IMPORT TOOL tool:local/mcp-register:latest AS mcp_register")
        command("IMPORT MESSAGE prompt:local/coder-soul:latest AS soul")
        command("IMPORT MESSAGE prompt:local/compact:latest AS compact")
        command("IMPORT TOOL tool:local/list-directory:latest AS list_dir")
        command("IMPORT TOOL tool:local/read-file:latest AS read_file")
        command("IMPORT TOOL tool:local/read-image:latest AS read_image")
        command("IMPORT TOOL tool:local/write-file:latest AS write_file")
        command("IMPORT TOOL tool:local/edit:latest AS edit")
        command("IMPORT TOOL tool:local/grep:latest AS grep")
        command("IMPORT TOOL tool:local/glob:latest AS glob")
        command("IMPORT TOOL tool:local/bash:latest AS bash")
        command("IMPORT TOOL tool:local/bash-exec:latest AS bash_exec")
        command("IMPORT TOOL tool:local/task-finish:latest AS task_finish")
        command("IMPORT TOOL tool:local/task-send:latest AS task_send")
        command("IMPORT TOOL tool:local/task-read:latest AS task_read")
        command("IMPORT SANDBOX sandbox:local/workspace-write:latest AS workspace_write")
        command("IMPORT SANDBOX sandbox:local/read-only:latest AS read_only")
        command("CREATE TEMP BLOCK msg (instructions, compact_prompt, compact_context)")
        command("CREATE TEMP BLOCK entry (pending)")
        command("CREATE BLOCK session (conversation)")
        command("INJECT soul TO msg.instructions")
        command("INJECT compact TO msg.compact_prompt")
        command("EFFECT sandbox_use ?", {config.sandbox})
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
            margatroid:wait(agent.address)
            return
        end
        command("INJECT [] TO entry.pending[0,1]")
        if message.type ~= "inject" then message = append_history(message) end

        if message.type == "inject" then
            inject_context(message)
        elseif message.type == "user" then
            command("INJECT ? TO session.conversation[-1]", {{message}})
            command("EFFECT start_turn")
            enqueue_inference()
        elseif message.type == "assistant" then
            if (message.content and message.content ~= "")
                or (message.reasoning and message.reasoning ~= "")
                or #(message.tool_calls or {}) > 0 then
                command("INJECT ? TO session.conversation[-1]", {{message}})
            end
            if #(message.tool_calls or {}) == 0 then
                maybe_compact_context(message)
                command("EFFECT finish_turn")
            else
                local results = command("EFFECT tool_call (session.conversation[-1].tool_calls)").Array.Message
                pending_tool_results = #results
                command("INJECT ? TO entry.pending[-1]", {results})
                maybe_compact_context(message)
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
