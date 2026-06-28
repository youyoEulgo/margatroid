# Margatroid Architecture

> The project is named after the Touhou character Alice Margatroid, the "Seven-Colored Puppeteer," who manipulates multiple dolls simultaneously. In Margatroid, the user is the AI puppeteer; the compose file defines each agent's capabilities and role; the delegation board is the thread connecting agents and user.

## 1. Design Philosophy

Margatroid adopts a **human team collaboration model** as its design foundation. A Workspace is a project group; each AI agent is a team member with specific skills; the Manager is the project manager; the delegation board is the team's collaboration infrastructure (TaskChain + publish area + SQLite archive).

## 2. Core Concepts

### 2.1 Workspace

An independent, sandboxed collaboration environment containing:
- A set of AI agent instances (defined by the compose file)
- A delegation board (Delegation Board + TaskChain)
- A SQLite database (worklog + personal_memory + schedule + delegations + conversation_messages)
- A shared sandbox environment

### 2.2 AI Agent Instance

Composed of the following elements:

| Element | Description |
|---|---|
| Provider + Model | Underlying AI model (OpenRouter / DeepSeek direct / Human) |
| Skills | The instance's capability set (function/tool definitions) |
| SOUL.md | Character identity and behavioral boundaries |
| Memory system | worklog + personal_memory |

### 2.3 AI Manager

Manager is a special member in the Workspace (Identity::Manager), equivalent to a project manager:
- Directly interfaces with the user, receiving user requirements
- Decomposes user requirements into structured delegatable tasks
- Distributes tasks to suitable agents via the delegation board
- Reviews returned results: finish if acceptable, finish with explanation if not
- Manager is itself an LLM-driven agent, with additional `schedule_*` tools

### 2.4 Delegation Board + TaskChain

The delegation board is the core collaboration infrastructure of a Workspace, driven by the TaskChain:

```
offer → chain append Delegate + publish area (frontend cache)
chain current_task() → member loop reads → executes if to matches
result(done=true) → chain append Outcome + publish removal + wake parent
```

**TaskChain** is a Turing-machine model — append-only, never delete or modify. Delegate shifts right, finish(done=true) shifts left. Context is the entire chain.

The **publish area** is downgraded to a frontend visualization cache; the member loop no longer depends on it. Scheduling is entirely driven by the chain's `current_task()`.

**Event-driven wakeup**: each member holds a `tokio::sync::Notify`. `offer()` and `result(done=true)` wake the target member after the chain head moves. No polling.

**User messages**: published as root delegation via `send_user_message("user", "manager", ...)`.

**Retry**: on failure, the task is automatically re-offered with `[RETRY:N]` prefix in detail; abandoned after 3 retries.

### 2.5 Schedule

Manager-specific phased task planning tool, stored in the board's SQLite.

| Operation | Effect |
|---|---|
| `schedule_add` | Add a planned entry |
| `schedule_list` | List all planned entries |
| `schedule_pop` | Pop the highest-priority entry for a member |
| `schedule_remove` | Delete a specified entry |

A phased task = a top-level work item assigned to a member. Phased tasks are published as delegations via the board and archived when the Manager accepts on completion.

### 2.6 Sandbox

OS-native sandboxing using kernel-level isolation mechanisms:

| Platform | Isolation Mechanism |
|---|---|
| Linux | Bubblewrap (`bwrap`) — mount namespace + PID namespace + network namespace |
| macOS | `sandbox-exec` — dynamically generated Seatbelt profile |

Writes are deny-by-default (allow-only); network is deny-by-default. Mandatory protected paths are hardcoded (~/.ssh, ~/.aws, .env, .gitconfig, etc.).

Each Workspace gets an independent sandbox. The sandbox environment is ephemeral and disposable; only workdir and memory.db are persisted.

## 3. Delegation Flow

```
1. Member A discovers they need B's capability
2. A identifies B as a likely fit via the team roster
3. A calls the delegate tool, specifying target, task_summary, task_detail,
   with work_summary/work_detail
4. board records A's outcome (done=false), chain appends sub-delegation, wakes B
5. B claims the task via chain current_task(), executes, calls finish on completion
6. board records B's outcome (done=true), chain head shifts left back to A, wakes A
7. A reviews B's result, decides to accept (finish) or flag issues (finish with explanation)
```

## 4. Human Interaction (User Members)

Users participate in the delegation chain as team members. Identity::User members use HumanProvider; Manager can delegate to users. HumanProvider creates requests via POST /api/human/request and blocks waiting for a reply. The frontend receives human_request events via the SSE stream and automatically switches the input box to reply mode. The chain resumes after the user submits a reply.

## 5. Tool-Call Loop (Streaming / Fallback)

`chat()` streams via `Client::chat_stream()`. For each chunk:
- Parse as `StreamChunk` (empty shell if parse fails); construct `WorkspaceEvent { content: StreamChunk { chunk } }`
- Push via `send_event()` to the `{workspace}/stream` EventBus channel
- Accumulate full_content / full_tool_calls / finish_reason / full_reasoning
- Non-streaming fallback: if StreamChunk parse fails, parse as `ChatResponse` (`message` instead of `delta`), merge into accumulators
- Save full text to conversation_messages after stream ends
- Non-breaking tools (bash/recall/schedule_*) → execute, continue loop
- finish → emit outcome (done=true), send chain_update event, break
- delegate → record partial outcome (done=false), publish new delegation, break

Streaming tool call arguments arrive in fragments. `merge_deltas` merges by `ToolCallDelta.index`.

## 6. Context Assembly

`Board.assemble_prompt(soul, memories)` assembles LLM context in fixed order:

```
1. System prompt (User)
2. Team roster (User)
3. Team worklog (User) — from memory cache
4. Delegation chain context (User)
5. Soul prompt (System) — SOUL.md
6. Personal memories (User)
7. Current task (User) — dynamic, always last
```

### Worklog Cache

SQLite writes in real time, but `DelegationBoard` holds an in-memory cache. Refreshed only at startup and on root delegation completion. Stable during sub-task execution to keep LLM context consistent.

## 7. Event System

Margatroid uses a global EventBus to manage event channels with naming pattern `<workspace>/stream`.

### 7.1 Unified Event Stream

The frontend subscribes via `GET /workspace/{name}/stream` as a persistent connection. All events flow through a single channel. Event structure is `metadata` + `content`:

```json
{
  "metadata": { "event": "stream_chunk", "member_id": "manager", "delegation_id": "...", "timestamp": 123 },
  "content": { "chunk": { "id": "...", "model": "...", "choices": [...] } }
}
```

Five event types:

| `metadata.event` | Trigger | `content` |
|---|---|---|
| `stream_chunk` | LLM streaming per chunk | `{ chunk: StreamChunk }` |
| `board_update` | offer / result(done=true) | `{ publish_count: number }` |
| `chain_update` | delegate shift right / finish shift left | `{ from, to, brief, head_pos }` |
| `member_status` | member start/end processing | `{ state: "working" \| "idle" }` |
| `human_request` | HumanProvider creates request | `{ session_id, from, to, brief, detail }` |

### 7.2 Rust Definitions

`types/src/events.rs`:

```rust
pub struct WorkspaceEvent {
    pub metadata: EventMetadata,  // event, member_id, delegation_id, timestamp
    pub content: EventContent,    // #[serde(untagged)] — no type tag
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum EventContent {
    StreamChunk { chunk: StreamChunk },     // references types::StreamChunk
    BoardUpdate { publish_count: usize },
    ChainUpdate { from, to, brief, head_pos },
    MemberStatus { state: String },
    HumanRequest { session_id, from, to, brief, detail },
}
```

### 7.3 EventBus

`runtime_v2/src/events.rs` — global `EventBus` (`HashMap<String, broadcast::Sender<String>>`). `Workspace::new()` registers the `"{name}/stream"` channel, and `Member::send_event()` constructs `WorkspaceEvent` and sends. No more three-channel model or event_bridge middle layer — the runtime directly constructs typed events.

### 7.4 Frontend Dispatch

The frontend switches on `metadata.event`; `content` maps directly to TypeScript types:

```ts
switch (metadata?.event) {
  case 'stream_chunk': handleStreamChunkEvent(content, metadata.member_id); break;
  case 'board_update': handleBoardUpdateEvent(content); break;
  case 'chain_update': handleChainUpdateEvent(content); break;
  case 'member_status': handleMemberStatusEvent(content, metadata.member_id); break;
  case 'human_request': handleHumanRequestEvent(content); break;
}
```

Five handlers, each receiving the corresponding type — zero processing direct pass-through.

### 7.5 Streaming UI

First `stream_chunk` immediately pushes a message; subsequent chunks append text in real time. `member_status: idle` flushes tool results and sets `loading = false`.

### 7.6 Emit Points

| Event | Trigger Location | Layer |
|---|---|---|
| stream_chunk | `member.chat()` while loop | runtime_v2 |
| board_update + chain_update | `trigger_event()` | runtime |
| chain_update | `member.chat()` should_break | runtime_v2 |
| member_status | `engine::member_loop` start/end | runtime_v2 |
| human_request | `human::handle_request` | server |

## 8. Memory System (SQLite)

Single `memory.db` file with five tables:
- **worklog** — team work log; inserted on delegation creation, completed on outcome
- **personal_memory** — per-member memory; on-demand keyword search
- **conversation_messages** — dialog messages; real-time write on each LLM text response
- **schedule** — Manager-specific schedule table
- **delegations** — delegation persistence

## 9. Provider Isolation

```
types::DynAiProvider   ← runtime holds via Client
types::AiProvider      ← providers implement
runtime::Client        ← wraps model + provider
```

Supported providers: `OpenRouterProvider`, `DeepSeekProvider` (direct API, OpenAI-compatible format), `HumanProvider`.

## 10. API Endpoints

| Method | Path | Description |
|---|---|---|
| GET | /health | Health check |
| POST | /v1/chat | Non-streaming chat |
| POST | /v1/stream | SSE streaming chat |
| GET | /v1/providers | AI provider list |
| POST | /admin/reload | Hot-reload providers |
| GET | /workspace | List running workspaces |
| POST | /workspace/{name}/chat | Send message to workspace |
| GET | /workspace/{name}/status | Board publish count |
| GET | /workspace/{name}/tasks | Full board status |
| GET | /workspace/{name}/events/{task_id} | Per-task SSE event stream |
| GET | /workspace/{name}/stream | Workspace unified event stream (persistent) |
| GET | /workspace/{name}/recent | Recent worklog |
| GET | /workspace/{name}/conversation | Conversation messages |
| POST | /api/human/request | Human interaction request |
| GET | /api/human/request/{id} | Blocking wait for human reply |
| GET | /api/human/requests | Pending request list |
| POST | /api/human/request/{id}/reply | Submit human reply |

## 11. Project Structure

```
margatroid/
├── types/         # Shared type definitions (DynAiProvider, AiProvider, Identity, ChatRequest, EventMetadata, EventContent)
│   ├── provider.rs  # DynAiProvider + AiProvider trait + blanket impl
│   ├── events.rs    # WorkspaceEvent = EventMetadata + EventContent (#[serde(untagged)])
│   └── event_index.rs # Event name constants
├── runtime/       # V1 core runtime
│   ├── board.rs   # DelegationBoard + TaskChain + assemble_prompt + trigger_event
│   ├── client.rs  # Client — wraps model + provider + streaming/fallback
│   ├── member.rs  # Member — Agent trait + chat() streaming tool-call loop
│   ├── memory.rs  # SQLite five tables
│   └── workspace.rs  # Workspace lifecycle + member_loop
├── runtime_v2/    # V2 runtime (gradually migrating)
│   ├── engine.rs  # member_loop (event-driven + send_event to EventBus)
│   ├── board.rs   # DelegationBoard V2 — pure scheduling layer
│   ├── member.rs  # Member V2 — Agent trait (holds EventBus ref)
│   ├── events.rs  # EventBus — HashMap<broadcast::Sender> channel registry
│   ├── workspace.rs # Workspace V2 — receives EventBus, manages member lifecycle
│   ├── context.rs # Context assembly (assemble_prompt + worklog)
│   ├── memory.rs  # SQLite
│   ├── tools.rs   # Tool execution
│   └── kernel.rs  # Kernel — multi-workspace process host
├── providers/     # LLM providers (OpenRouter + DeepSeek + Human)
├── compose/       # Compose file parsing
├── assets/        # Member library (member.toml + SOUL.md)
├── sandbox/       # Sandbox execution environment
├── cli/           # CLI entry point
└── server/        # HTTP API (axum) + SSE + workspace routing
    ├── human.rs        # Human interaction endpoints
    └── handlers/       # stream, chat, workspace, providers, admin
```

## 12. Design Decisions

1. **Append-only TaskChain** — Delegate and Outcome entries are never modified after write; the chain is the single source of truth for scheduling and context
2. **Event-driven** — Notify + chain-head detection replaces polling, zero CPU busy-wait
3. **metadata.event dispatch + content untagged** — frontend switches on metadata.event; content is bare data mapping directly to TypeScript types, zero processing
4. **Single-channel EventBus** — deprecated three-channel model + event_bridge; one `{name}/stream` channel per workspace; runtime directly constructs typed JSON
5. **Worklog cache** — refresh at startup and root completion only; stable during sub-task execution for prompt cache
6. **Manager is an agent** — not a hardcoded scheduler
7. **Publish area downgraded** — frontend cache, decoupled from scheduling logic
8. **Two-tier member discovery** — team roster (lightweight context injection) + recall on-demand retrieval
9. **Four-level logging** — error/warn default on; info for business events; debug for raw traffic
10. **Identity routing** — CLI dispatches by `match identity` in three independent branches
11. **Human member** — User paired with HumanProvider; the user is both chain initiator and delegatable target
12. **Streaming UI** — first chunk pushes message immediately; subsequent chunks append in real time; idle flushes tools and sets loading
