# 持久 Bash

空参数启动终端；收到私有 ready 后返回 `{task_id}`，任务持续运行。输入必须是含字符串 `text` 和可选布尔 `enter` 的对象，`enter=true` 追加 CR。output 的 replace 映射 task_replace，否则非空文本 task_write；最终追加 `{exit_code}` 再 finish。任务超时为 0。

资源的 `latest/shell.lua` 在执行阶段以 `dofile(context.resource_dir .. "/shell.lua")(context)` 加载，只调用通用 `process_open`、任务和 IO。每个资源独立携带同版本 helper 与预编译 `latest/bin/shell-client`；打包方复制 binary，Lua 不下载或编译。工作目录由宿主固定为项目根。

私有 stdin/stdout 使用 JSONL，stderr 诊断持续独立消费并舍弃。helper 累积跨块数据并按换行分帧，支持同块多帧。writer 仅在整帧完成后出队，`await_any` 取消后重试同一完整帧。最终 result/exit 必须先消费；stdout、stderr 都到 EOF 后才 wait，不以提前可用的 wait 抢走尾帧。宿主原始累计字节预算仍包含协议和转义开销。

内部 API 为 `client.exec(command, {max_output_bytes, pty?})` 和 `client.terminal({max_output_bytes})`；terminal 提供 `read()`、`write(text)`、`close()`。write 排队 `{operation:"write",text}`；close 发送 `{operation:"close"}` 后持续读取两条管道并等待 binary 清理退出。PTY 布尔选项原样交给 binary，默认 false。exec 默认 16 MiB，terminal 明确默认 128 MiB；`timeout_ms` 在 helper 中拒绝，超时只由 `configure` 声明。协议或进程错误向上传播，现有任务生命周期负责清理。
