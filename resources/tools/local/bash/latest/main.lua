local MAX_OUTPUT_BYTES = 16 * 1024 * 1024

-- The command's output goes to the model as-is, so the model reads the text
-- rather than a JSON envelope. `ok` carries the one thing the text cannot say:
-- whether the runtime, the interface and the history should treat this call as
-- a failure. A non-zero exit code is a failure here, the way a shell reports it.
function execute(arguments, context)
    local result = margatroid().shell(arguments.cmd, {
        max_output_bytes = MAX_OUTPUT_BYTES,
    })

    -- A tool that can answer at all must answer with something the model can act
    -- on. A silent success is fine; a silent failure is not, because the model
    -- would have nothing to reason about and would have to guess why it failed.
    local content = result.stdout
    if content == "" then
        if result.exit_code == 0 then
            content = "command completed with no output"
        else
            content = string.format("command exited with code %s and no output", tostring(result.exit_code))
        end
    elseif result.truncated then
        content = content .. "\n[output truncated at the size limit]"
    end

    return margatroid().json.encode({
        ok = result.exit_code == 0,
        content = content,
    })
end
