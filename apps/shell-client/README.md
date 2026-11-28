# shell-client

`shell-client` 为工具资源提供一次性 shell 执行和交互终端进程。
它继承调用方工作目录与环境，通过 stdin/stdout 交换 JSON Lines，诊断写入 stderr。
普通 `exec` 适用于 Unix，`pty: true` 与交互 `terminal` 适用于 Linux。
调用资源见 [bash](../../resources/tools/margatroid/bash/dev/main.lua) 和
[bash-exec](../../resources/tools/margatroid/bash-exec/dev/main.lua)。

## 构建与协议入口

从 workspace 根目录构建：

```sh
cargo build --release -p shell-client
```

产物为 `target/release/shell-client`，安装器将其复制到 bash 与 bash-exec 的 `dev/bin`。
调用方建立进程管道，每次发送一个以换行结束的 UTF-8 JSON 对象，并逐行读取响应。
首个请求选择 `exec` 或 `terminal`；调用方保持 stdin 开启直至操作结束或主动关闭。

## 一次命令执行

向 stdin 写入一行：

```json
{"operation":"exec","command":"printf hello","max_output_bytes":65536,"pty":false}
```

`exec` 通过 `/bin/sh -c` 执行原始 command，继承工作目录与环境。
需要 Bash 语法时，在 command 中显式调用 `bash --noprofile --norc -c '…'`。
完成后返回一行结果并退出：

```json
{"type":"result","stdout":"hello","stderr":"","code":0,"exit_code":0,"signal":null,"truncated":false,"terminal":false}
```

stdout、stderr 各保留至多 `max_output_bytes` 个原始字节，超限部分继续排空，
结果标记 `truncated: true`。UTF-8 解码使用替换字符处理非法字节。
Linux 的 `pty: true` 使用真实 PTY，合并输出至 stdout，并返回 `terminal: true`。
`code` 和 `exit_code` 在信号终止时为 `-1`，`signal` 返回 Unix 信号编号。

## 一次交互终端

Linux 上发送初始请求，收到 `ready` 后发送原始输入：

```jsonl
{"operation":"terminal","max_output_bytes":65536}
{"operation":"write","text":"printf hello; exit\r"}
```

终端以 24 行 × 80 列 PTY 启动 `bash --noprofile --norc -i`，设置 `TERM=xterm-256color`。
输出帧为 `{"type":"output","text":"…","replace":false}`；调用方在 `replace: true`
时替换完整转录，其余情况追加 text。`write.text` 原样写入，调用方自行提供回车字符。
结束帧为 `{"type":"exit","eof":true,"exit_code":0}`，信号终止时 `exit_code` 为 null。

`TerminalOutput` 用 VT100 解析器维护文本历史与当前屏幕，备用屏按物理行导出。
每次读取 PTY 数据时同步实际行列数，尺寸改变后的首个输出使用完整替换。
单元格滚屏容量随实际行数设为两倍；文本历史与屏幕受 `max_output_bytes` 约束，
超过预算返回 error。直接 shell 退出后排出当时 PTY 缓冲区已有的尾部字节，再发送 exit。

## 关闭与错误处理

后续输入 `{"operation":"close"}`、stdin EOF，或 Unix 的 SIGTERM、SIGINT、SIGHUP，
触发直接 child 及其进程组的终止与等待。逃出进程组的后代由外层工具 supervisor 管理。
正常 exec 完成条件包括直接 child 退出及输出管道排空。
错误通过 `{"type":"error","message":"…"}` 返回，并以非零进程状态退出。
请求选择 PTY 时要求 Linux；其他平台返回 `PTY requires Linux` 错误。

## 源码与测试导航

- [main.rs](src/main.rs)：`Request` 输入类型、exec、管道捕获、进程组清理与唯一 stdout writer。
- [pty.rs](src/pty.rs)：Linux PTY 建立、并发读写、实际尺寸读取、尾部排空与终端退出。
- [output.rs](src/output.rs)：`TerminalOutput` 的 VT100 转录、追加/替换决策、尺寸与预算，以及对应单元测试。
- [protocol.rs](tests/protocol.rs)：真实管道执行、Bash 包装、截断、PTY、交互输出和进程清理。

测试入口为 `cargo test -p shell-client`，交互测试使用 Linux、Bash 与实际 PTY。
