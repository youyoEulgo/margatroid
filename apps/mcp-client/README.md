# mcp-client

`mcp-client` 是供工具资源调用的 MCP 协议进程。一个进程承载一次服务注册，
将调用方的 JSON Lines 请求转换为 MCP JSON-RPC，通过 stdio 或 Streamable HTTP 访问服务。
Agent 配置和工具别名由调用它的
[mcp-register 资源](../../resources/tools/margatroid/mcp-register/dev/main.lua)处理。

## 构建与进程连接

从 workspace 根目录构建：

```sh
cargo build --release -p mcp-client
```

产物为 `target/release/mcp-client`。安装器将 helper 放入 mcp-register 的 `dev/bin`，
资源从该位置启动进程，建立原始 stdin/stdout 管道并独立消费 stderr。
每个输入和输出帧都是以换行结束的 UTF-8 JSON 对象，stdout 承载响应，stderr 承载诊断。
调用方在会话期间保持 stdin 开启，结束时关闭输入并等待进程退出。

## 一次注册与调用

先发送 `register`，等待对应响应后再发送 `call`。以下输入使用
[测试服务](tests/fixtures/server.py)，路径相对 workspace 根目录：

```json
{"id":"register","operation":"register","server":{"transport":"stdio","command":"python3","args":["apps/mcp-client/tests/fixtures/server.py"],"env":{"MCP_FIXTURE_ENV":"present"}}}
```

注册依次执行 `initialize`、`notifications/initialized` 和完整分页 `tools/list`，
以 `result.tools` 返回原始工具定义。注册完成后可发送：

```json
{"id":1,"operation":"call","tool":"echo","arguments":{"text":"hello"}}
```

响应携带原请求 `id`，成功值位于 `result`，JSON-RPC 错误位于 `error`。
远端工具结果中的 `isError` 保留在 `result` 内，远端错误的 code、message、data 原样保留。
`id` 使用当前活动请求间唯一的字符串或整数；并发响应按实际完成顺序到达。
`arguments` 默认 `{}`，类型要求为对象，`tool` 使用远端原始名称。
stdio 的 `args`、`env` 默认为空；`env` 条目覆盖继承环境中的同名变量。
重复注册返回错误，重试注册使用新进程。

HTTP 服务通过同一入口选择传输：

```json
{"id":"register","operation":"register","server":{"transport":"http","url":"http://127.0.0.1:3000/mcp","headers":{"Authorization":"Bearer example"}}}
```

HTTP 接受 http/https URL，TLS 使用 native roots；请求携带 JSON 与 SSE Accept 类型，
初始化后携带协商版本及服务返回的 session id。POST 响应支持 JSON 和增量 SSE，
匹配响应到达即完成当前请求。通知 POST 以 HTTP 202 确认。
协议提出版本 `2025-06-18`，接受 `2024-11-05`、`2025-03-26`、`2025-06-18`、`2025-11-25`。

## 会话边界与错误

单帧和单个 SSE event 预算为 128 MiB，发现目录总预算为 64 MiB，
本地与远端活动请求、stdio 写队列的容量各为 8192。
JSON 解析错误返回 null-id 错误帧；重复活动 id、超限或中断的输入帧触发会话清理。
远端进程退出、读写失败或协议错误会使所有等待中的远端请求返回错误。

stdin EOF 取消活动调用并等待会话任务结束；stdio 服务的直接子进程被终止并等待，
Unix 下同时终止其专属进程组。调用方通过资源层管理期限和取消策略。

## 源码与测试导航

- [main.rs](src/main.rs)：`Server` 描述传输；`Session` 管理连接、请求映射、初始化与关闭；`operation` 分派注册和调用，`main` 管理 JSON Lines。
- [protocol.rs](tests/protocol.rs)：真实子进程管道、分页工具发现、并发乱序响应、远端错误、HTTP JSON/SSE 和 EOF 清理。
- [server.py](tests/fixtures/server.py)：Python 标准库实现的 stdio 与 HTTP 测试服务。
- [host_integration.rs](tests/host_integration.rs)：Linux 下通过真实 Lua 资源和 ToolPlugin 验证并发路由与 owner 清理。

协议测试使用 `cargo test -p mcp-client --test protocol`，测试服务依赖 `python3`。
传输规范参考 [MCP transports](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports)。
