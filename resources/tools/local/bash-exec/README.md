# Bash 单次命令

[入口](latest/main.lua) 与 [helper](latest/shell.lua) 使用当前的命名空间 API。

保持 `{cmd}` 参数和 120000 毫秒任务超时。原命令以原有单引号转义后传给 `bash --noprofile --norc -c`，外层由独立 binary 的 `/bin/sh -c` 执行。输出上限仍为 64 KiB，stdout、`[stderr]`、无输出提示、truncated 提示和 exit code 的结果字节拼接沿用原实现。

资源的 helper 仍在执行阶段以 `dofile(context.resource_dir .. "/shell.lua")(context)` 加载。通用接口为 `margatroid.process:start(program, args)`、`margatroid:wait_any`、`margatroid.data:encode` / `decode`（JSON 格式）；结果使用 `margatroid.task:return_result`，随后 `margatroid.task:finish(context.task_id)`。入口参数取自 `margatroid.context.arguments`，元数据取自 `margatroid.context`，不依赖根对象的 arguments 或 version。

Bash-exec 继续依赖 shell-client，保留原生进程协议及 PTY 选项。每个资源独立携带同版本 helper 与预编译 `latest/bin/shell-client`；打包方复制 binary，Lua 不下载或编译。工作目录由宿主固定为项目根。

私有 stdin/stdout 使用 JSONL，stderr 诊断持续独立消费并舍弃。helper 累积跨块数据并按换行分帧，支持同块多帧。writer 仅在整帧完成后出队，`margatroid:wait_any` 取消后重试同一完整帧。最终 result/exit 必须先消费；stdout、stderr 都到 EOF 后才 wait，不以提前可用的 wait 抢走尾帧。宿主原始累计字节预算仍包含协议和转义开销。

内部 API 为 `client.exec(command, {max_output_bytes, pty?})` 和 `client.terminal({max_output_bytes})`；terminal 提供 `read()`、`write(text)`、`close()`。write 排队 `{operation:"write",text}`；close 发送 `{operation:"close"}` 后持续读取两条管道并等待 binary 清理退出。PTY 布尔选项原样交给 binary，默认 false。exec 默认 16 MiB，terminal 明确默认 128 MiB；`timeout_ms` 在 helper 中拒绝，超时只由 `configure` 声明。协议或进程错误向上传播；宿主自动记录原始诊断，对外返回简洁错误，现有任务生命周期负责清理。
