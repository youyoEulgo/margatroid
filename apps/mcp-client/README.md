# mcp-client

An independent MCP protocol host. It knows nothing about Agents, tasks, Lua, ECS,
resource installation, or tool aliases. One process owns at most one server connection.
It never downloads or installs a server or another executable.

Build from the workspace root with `cargo build --offline --release -p mcp-client`.
The artifact is `target/release/mcp-client`; the resource author delivers that binary
as `mcp-register/latest/bin/mcp-client`. The host does not build it automatically.
Run `cargo test --offline -p mcp-client` for integration tests. These require `python3`
and use only Python's standard library, real child pipes, and loopback HTTP.
On Linux, `cargo test --offline -p mcp-client --test host_integration` also runs the
actual bundled Lua resource through App/ToolPlugin, copies the built MCP client into
a temporary resource, checks concurrent routing, and verifies owner cleanup. Its
custom test executable reuses the public tool runner entry in supervisor/worker mode;
there is no extra product binary or dependency on a prebuilt external tool runner.

## Private JSON Lines protocol

Use raw stdin/stdout pipes, with no PTY. Every request and response is a single UTF-8
JSON object terminated by a newline. stdout contains only response objects. stderr
contains diagnostics and server notifications; consume it independently. Each request
must have a unique active string or integer `id`. Responses copy that id exactly and
may arrive in any order. Do not close stdin until you intend to tear down the session:
EOF aborts unfinished requests, kills and waits for the stdio child, kills its dedicated
Unix process group (including descendants), and exits. No hidden request deadline is
imposed. This is the actual final wire format:

```json
{"id":"register-1","operation":"register","server":{"transport":"stdio","command":"python3","args":["server.py"],"env":{"EXAMPLE":"value"}}}
{"id":"register-1","result":{"tools":[{"name":"echo","description":"Echo text","inputSchema":{"type":"object","properties":{"text":{"type":"string"}}}}]}}
{"id":"call-1","operation":"call","tool":"echo","arguments":{"text":"hello"}}
{"id":"call-1","result":{"content":[{"type":"text","text":"hello"}],"isError":false}}
{"id":"call-2","error":{"code":-32602,"message":"Remote validation failed","data":{"detail":"example"}}}
```

The `env` and `args` fields default to empty. Environment entries override inherited
process variables. A second register operation is rejected even when the first fails;
start a new client process to retry. Calls before completed registration are rejected.
`arguments` defaults to `{}` and must be an object. `tool` is the original remote tool
name. Tool definitions and tools/call results are returned intact; in particular a
remote `isError: true` remains inside `result`, while JSON-RPC errors appear in `error`
and retain their remote code, message, and data. Discovery follows all `nextCursor`
pages. Register returns only after initialize, initialized notification, and discovery.

For Streamable HTTP use the same register operation with this server object:

```json
{"id":"register-1","operation":"register","server":{"transport":"http","url":"http://127.0.0.1:3000/mcp","headers":{"Authorization":"Bearer example"}}}
```

HTTP URLs must use http or https. TLS uses native roots. Redirects are disabled.
Requests include `Accept: application/json, text/event-stream`, JSON Content-Type,
and, after initialization, negotiated `MCP-Protocol-Version` and any server-issued
`Mcp-Session-Id`. Notifications require HTTP 202. Request POSTs support both ordinary
JSON responses and incrementally decoded SSE events (comments, CRLF, fragmented UTF-8,
and multiline `data:`). The matching response completes a POST SSE reader immediately;
it need not wait for the server to close a persistent stream. Multiple POSTs run
concurrently. HTTP response bodies and headers are never logged.

The implementation uses standard MCP JSON-RPC 2.0, proposes version `2025-06-18`, and
accepts negotiation to `2024-11-05`, `2025-03-26`, `2025-06-18`, or `2025-11-25` for this
basic tools subset. It advertises no optional client capabilities. Server `ping`
requests are answered; unsupported server requests receive method-not-found. Server
notifications are accepted without generating private stdout frames. The optional
standalone GET notification stream, SSE reconnection/replay, DELETE session cleanup,
legacy HTTP+SSE transport, OAuth flows, and private cancel operation are not implemented.
A server needing those extra features requires an expanded client implementation.

## Budgets and failure behavior

Frames and individual SSE events have a 128 MiB limit. Tool discovery has a 64 MiB aggregate
budget, repeated pagination cursors are rejected, and local and remote in-flight requests
and the stdio write queue are limited to 8192. The stdout writer is unique and its queue
is bounded. Oversized responses produce an explicit error rather than a truncated result.
Invalid JSON produces a null-id parse error; invalid ids produce a null-id invalid-request
error. Duplicate active local ids or an oversized/incomplete input frame terminate the
session with a stderr diagnostic because safe per-request attribution is impossible.

Stdio writes use a separate bounded writer and never block the server reader. Remote
EOF, malformed JSON-RPC, server process exit, and write/read failure fail every pending
remote request. Unknown remote response ids are discarded with a diagnostic. Server
stderr is inherited directly. HTTP request/stream failure is reported to the matching
local request. EOF cleanup cancels HTTP work by dropping requests; it does not wait for
an optional server DELETE response. There is no synthesized timeout; the calling Lua
resource owns its time and cancellation policy.

Transport reference: <https://modelcontextprotocol.io/specification/2025-06-18/basic/transports>.
