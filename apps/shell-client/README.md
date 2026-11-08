# shell-client

独立 shell/PTY 可执行程序，使用当前工作目录及继承的环境。只依赖通用操作系统、异步 IO、JSON 和 VT100 库。stdout 仅输出 JSONL，诊断写 stderr。无需下载、安装或用户数据目录。

启动后 stdin 第一行选择模式：

```json
{"operation":"exec","command":"printf hello","max_output_bytes":65536,"pty":false}
```

以 `/bin/sh -c` 原样执行 command。Bash 特有语法由调用方显式包装 `bash --noprofile --norc -c '...'`。完成后输出一行并退出：

```json
{"type":"result","stdout":"hello","stderr":"","code":0,"exit_code":0,"signal":null,"truncated":false,"terminal":false}
```

stdout、stderr 分别保留最多 max_output_bytes 原始字节，超出后继续 drain，设置 truncated。PTY 模式输出合并到 stdout。无效 UTF-8 使用替换字符，与原 shell 捕获行为一致。

```json
{"operation":"terminal","max_output_bytes":65536}
```

Linux 创建 24×80 PTY 并启动 `bash --noprofile --norc -i`，TERM 为 xterm-256color。先输出 `{"type":"ready"}`，随后连续输出 `{"type":"output","text":"...","replace":false}`。调用方按 replace 决定替换或追加整个当前转录。普通屏幕保留滚动历史，alternate screen 使用当前屏幕。屏幕或历史超预算输出 error，不丢弃旧行。

后续 stdin 接受 `{"operation":"write","text":"ls\r"}` 或 `{"operation":"close"}`。write 原样发送，不添加回车。直接 shell 退出后，仅排出当时 PTY 缓冲区已有的尾部字节，不等待持有 slave 的后台进程关闭。结束帧为 `{"type":"exit","eof":true,"exit_code":0}`，信号终止时 exit_code 为 null，exit 是最后一帧。

调用方应在等待结果期间保持 stdin 开启。close、stdin EOF、SIGTERM、SIGINT 或 SIGHUP 会终止直接 child 和其进程组并 wait；不设置隐藏超时。exec 正常完成不会杀 detached 后代。逃出进程组的后代由外层进程监督者管理。程序不建立 daemon 或全局进程管理器。

错误输出 `{"type":"error","message":"..."}` 后以非零状态退出。PTY 在非 Linux 平台明确返回 unsupported；普通 exec 支持 Unix。单个 writer 顺序写出所有协议帧，使用有界队列传递输出；PTY 读写并发，不在 child 锁内等待。

验证：`cargo test -p shell-client --offline`。


当前依赖边界：list-directory、glob、grep、write-file改用通用files、text和data能力，不再携带shell.lua或本bin。Bash和Bash Exec继续使用本程序承担实际shell、PTY及终端行为。文件工具不再打包本bin，Shell协议和终端行为保持原有语义。
