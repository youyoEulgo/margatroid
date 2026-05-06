# Margatroid Architecture

> The project name comes from the Touhou Project character Alice Margatroid, known as the "Seven-Colored Puppeteer," capable of controlling multiple dolls simultaneously. In this project, the user is the puppeteer, the compose file defines each agent's capabilities and role, and the Delegation Board is the thread connecting agents to the user.

## 1. Design Philosophy

Margatroid's initial inspiration came from Docker Compose — users declare a configuration file to orchestrate multiple AI agent instances, enabling them to collaborate on complex tasks. However, during the design process, we found a fundamental mismatch in the container orchestration metaphor: containers communicate through request-response networking, while agent collaboration more closely resembles division of labor and delegation in a human team.

We therefore abandoned the purely engineering metaphor and adopted the **human team collaboration model** as our design foundation. A Workspace is a project team, each AI agent is a team member with specific skills, the Manager is the project lead, and the Delegation Board is the team's collaboration infrastructure.

## 2. Core Concepts

### 2.1 Workspace

A Workspace is an isolated, sandboxed collaboration environment containing:

- A set of AI agent instances (defined by a compose file)
- A shared working directory (workdir)
- An AI Manager (project lead)
- A Delegation Board
- Independent memory storage per agent (SQLite)

Workspaces are fully isolated from each other, each with its own sandbox environment.

### 2.2 AI Agent Instance

An agent instance is a "team member" defined in the compose file, composed of:

| Element          | Description                                                                |
| ---------------- | -------------------------------------------------------------------------- |
| Provider + Model | The underlying AI model (e.g., a model on OpenRouter)                      |
| Skills           | The set of capabilities the instance possesses (function/tool definitions) |
| System Prompt    | Defines the member's role identity and behavioral boundaries               |
| Profile          | Detailed capability description file, queried by other members             |
| Memory DB        | Independent SQLite database storing the member's work memory               |

Each instance runs within the Workspace sandbox, can only access the workdir and Delegation Board, and cannot communicate directly with other instances.

### 2.3 AI Manager

The Manager is a special member of the Workspace, equivalent to a project lead:

- Directly interfaces with the user, receiving user requirements
- Decomposes user requirements into delegatable structured tasks
- Distributes tasks to appropriate agents via the Delegation Board
- Handles exceptions and disputes during delegation (e.g., an agent unable to complete a task, results being rejected)
- The Manager itself is also an LLM-powered agent, not a hard-coded scheduler

### 2.4 Delegation Board

The Delegation Board is the core collaboration infrastructure of the Workspace — an **event-driven task queue**. Its operation is analogous to the Tokio runtime:

- Any member (including the Manager and regular agents) can post delegation tasks to the Board
- A delegation task contains: target member ID, task description, structured parameters, priority, deadline
- The Board maintains a status flag for each member (idle / working)
- When the target member is idle, the Board immediately assigns the task
- When a task completes or fails, the Board notifies the initiator
- Event-driven, not polling — members actively notify the Board upon completion, the Board wakes and scans the pending queue

**User communication also flows through the Board.** User messages to the Manager are posted as tasks with `PRIORITY_USER` (u32::MAX), the highest possible priority. When the user changes direction, the Manager calls `cancel()` on the current task — the Board marks it as Interrupted and releases the target member.

**Recall requests also use the Board, but skip the worklog.** When Member A wants to query Member B's past experience, they post a lightweight delegation with `skip_worklog: true`. B searches their personal memory and returns a summary. Unlike normal delegations: no worklog entry, not rejectable, and does not occupy B's Working state longer than necessary.

### 2.5 Sandbox

Margatroid uses an **OS-native sandbox approach** (inspired by Claude Code's `sandbox-runtime`), without Docker or VMs, leveraging OS kernel-level isolation primitives:

| Platform | Isolation Mechanism | Dependency |
|----------|---------------------|------------|
| Linux | Bubblewrap (`bwrap`) — mount namespace + PID namespace + network namespace | Requires `bubblewrap` |
| macOS | `sandbox-exec` — dynamically generated Seatbelt profiles | Built into the OS |

**Dual isolation model.** Filesystem isolation and network isolation must exist together — neither alone is sufficient. Filesystem isolation alone leaves data exfiltration via network; network isolation alone leaves system files vulnerable.

**Filesystem rules.** Write access is allow-only (denied by default); read access is deny-then-allow (allowed by default, with sensitive regions blocked). Only the workdir, `/tmp`, and Margatroid data directories are writable. All host system binaries (rustc, gcc, node, python, etc.) are visible as read-only — agents inherit the host toolchain directly without needing to install anything inside the sandbox.

**Network rules.** All network access is denied by default (allow-only). All HTTP/HTTPS traffic goes through an HTTP proxy for filtering; all other TCP traffic goes through a SOCKS5 proxy, both enforcing domain allowlists. Proxies run on the host outside the sandbox; in-sandbox processes connect to them via Unix Domain Sockets (Linux) or localhost ports (macOS).

**Mandatory deny paths.** The following paths are hard-coded as non-writable in the source code, not overridable by any configuration: `~/.ssh/`, `~/.aws/`, `.env`, `.gitconfig`, `.git/hooks/`, `.mcp.json`.

**Ephemeral by default.** The sandbox environment is ephemeral — when a Workspace stops, the sandbox is destroyed and starts fresh on the next launch. Only the workdir and memory.db are persistently mounted and survive across sessions. Agents can freely install system packages (`apt-get`, `pip install`, etc.) inside the sandbox; these changes are automatically discarded on restart.

**One sandbox per Workspace.** All agent instances run within the same sandbox, sharing filesystem and network rules. Different Workspaces are fully isolated from each other.

## 3. Compose File Specification

The compose file is the declarative definition of a Workspace, using the TOML format (differing from Docker Compose's YAML convention, as Margatroid uses TOML throughout for configuration).

### 3.1 Minimal Example

```toml
[workspace]
name = "my-project"
version = "0.1.0"
description = "An example project team"
workdir = "./project"

[[agents]]
id = "architect"
provider = "OpenRouter"
model = "anthropic/claude-sonnet-4"
system_prompt = "You are a software architect, responsible for designing system architecture and making technical decisions."
skills = ["design", "code-review"]
depends_on = []

[[agents]]
id = "coder"
provider = "OpenRouter"
model = "google/gemini-2.5-flash"
system_prompt = "You are a programmer, responsible for writing code based on architectural designs. When encountering architecture questions, delegate to the architect."
skills = ["coding", "testing"]
depends_on = ["architect"]

[[agents]]
id = "reviewer"
provider = "OpenRouter"
model = "moonshotai/kimi-k2.6"
system_prompt = "You are a code reviewer, responsible for reviewing code quality and security."
skills = ["code-review", "security-review"]
depends_on = []
```

### 3.2 Field Reference

**Workspace top-level:**

| Field       | Type   | Required | Description                                                  |
| ----------- | ------ | -------- | ------------------------------------------------------------ |
| name        | string | Yes      | Unique Workspace identifier                                  |
| version     | string | Yes      | Configuration version number                                 |
| description | string | No       | Project description                                          |
| workdir     | string | Yes      | Trusted project directory, relative to compose file location |

**Agent list items:**

| Field         | Type     | Required | Description                                                                                     |
| ------------- | -------- | -------- | ----------------------------------------------------------------------------------------------- |
| id            | string   | Yes      | Unique member identifier                                                                        |
| provider      | string   | Yes      | AI provider name, corresponding to a provider in margatroid.toml                                |
| model         | string   | Yes      | Model ID                                                                                        |
| system_prompt | string   | Yes      | Role definition and basic behavioral boundaries                                                 |
| skills        | string[] | Yes      | List of capability tags                                                                         |
| depends_on    | string[] | No       | Declarative dependencies, for self-documenting the collaboration topology                       |
| profile       | string   | No       | Path to detailed capability description file (auto-generated default template if not specified) |
| max_tokens    | u32      | No       | Maximum token count per request                                                                 |
| temperature   | f32      | No       | Model temperature parameter                                                                     |

**Note:** The `depends_on` field is currently only for self-documentation; it does not enforce startup ordering. Collaboration relationships are dynamically established through the Delegation Board.

## 4. Team Collaboration Model

### 4.1 Member Discovery Mechanism

Each member learns about the team through a two-layer information structure:

**Layer 1: Public Roster Skill**

Generated from the compose file and injected into each member's available skill list. Contains basic information about all members:

```
Team members:
- architect: Software architect, skilled in system design and code review
- coder: Programmer, skilled in coding and testing
- reviewer: Code reviewer, skilled in code review and security review
```

This is a lightweight "team directory" — each member knows who is on the team and roughly what they do at startup.

**Layer 2: Individual Profile**

Each member holds a detailed Profile file recording specific capability scope, technical stack expertise, work preferences, etc. When Member A determines via the roster that a task might suit Member B, they query B's full Profile through the Delegation Board, then initiate delegation after confirmation.

This two-layer design avoids stuffing every member's detailed capabilities into each agent's context window. The Roster acts as a Bloom filter — quickly identifying "who might be able to help" — and the Profile is loaded only after confirming the candidate.

### 4.2 Delegation Flow

```
1. Member A discovers during task execution that Member B's capability is needed
2. A identifies B as a suitable candidate via the roster skill
3. A queries B's availability and detailed Profile through the Delegation Board
4. A confirms delegation and submits a structured task to the Delegation Board
5. The Board checks B's status (idle → assign / working → enqueue)
6. B receives the task, executes it, and notifies the Board upon completion
7. The Board returns the result to A
8. A validates the result (satisfied → continue / unsatisfied → re-delegate or escalate to Manager)
```

### 4.3 Dispute Resolution

When Member A is unsatisfied with Member B's execution result:

- A can reject the result with a reason, requesting B to re-execute
- If rejections exceed the threshold (default: 1), the Board automatically escalates the dispute to the Manager
- The Manager intervenes to arbitrate: choose a replacement executor, re-decompose the task, or handle it personally

### 4.4 Manager Task Decomposition

Upon receiving a user requirement, the Manager:

1. Queries the roster on the Delegation Board to understand current team capabilities
2. Decomposes the requirement into a structured subtask DAG
3. Subtasks must be combinations of known skill names and structured parameters — the Board can validate legality
4. Publishes subtasks to the Delegation Board in topological order, specifying executors
5. Monitors execution progress and handles exceptions

## 5. Memory Architecture

Margatroid's memory system mirrors how real human teams document their work, split into two layers:

### 5.1 Worklog

**Team-shared handoff log.** Each member, upon completing a delegation, auto-summarizes their work into a short entry appended to the shared worklog. Each entry is ~30-50 tokens: who, what they did, what they produced, what's left unresolved.

The worklog is a **fixed prefix** in every agent's request — system prompt + roster + recent worklog entries + agent personality + current delegation. It is always injected (not queried on-demand), ensuring every member has a baseline awareness of what the team is doing.

### 5.2 Personal Memory

**Each member's private notebook.** After completing a delegation, alongside the worklog entry, the agent saves the delegation's full context (complete conversation, code changes, decisions and reasoning, problems encountered and solutions) to their own `memory.db`. This is an unbounded appendix, but is never actively injected into the prompt.

### 5.3 Recall Mechanism

**Query another member's past experience through the Board.** Recall is designed as a skill — every agent has it by default. When Member A, during a delegation, needs Member B's past experience:

```
A invokes recall_skill(target="B", query="how was that bug fixed last time") →
  Posts a skip_worklog delegation via the Board to B →
    B searches their memory.db, returns a summary →
      Summary is injected into A's prompt; A continues
```

**Recall does not write to the worklog.** Just like walking over to a colleague's desk to ask a question — no meeting notes needed. The Board routes the request without producing a persistent entry.

**Recall complements, not replaces, the worklog.** The worklog is passive awareness — every member knows "what the team is working on." Recall is active lookup — triggered only when specific details are needed.

### 5.4 Storage Layout

```
{workspace_root}/
├── worklog.db              # Team worklog (shared)
└── {agent_id}/
    └── memory.db           # Personal memory (private)
```

- **worklog.db** — Append-only. Each entry: timestamp, agent_id, delegation_id, summary text. All agents can read; no write conflicts (each agent only writes their own completion records).
- **memory.db** — Independently managed per agent. Phase 3 starts with an in-memory implementation; Phase 4 introduces SQLite + FTS5 full-text search.

### 5.5 KV Cache Optimization

Each request's prompt structure follows a fixed order to maximize cache hit rates:

```
[System prompt] [Roster] [Worklog ← fixed prefix, shared by all agents]
[Agent personality prompt ← static, reused across requests for same agent]
[Current delegation details ← dynamic zone]
[If needed: recall results ← appended on demand, does not disrupt prefix structure]
```

The first two segments are shareable across all requests within a workspace. The role prompt is reusable across different delegations for the same agent.

## 6. System Architecture

### 6.1 Crate Structure

```
margatroid/
├── types/          # Shared type definitions (request, response, config, message, Tool, MCP, Bridge protocol)
├── paths/          # Path layout management (root, workspace, config, data)
├── assets/         # Unified asset manager (app config + workspace lifecycle)
├── providers/      # AI provider adaptation layer (trait + OpenRouter implementation)
├── server/         # HTTP service (Axum)
├── bridge/         # Claude Code Remote Control protocol client
├── cli/            # Command-line interface (margatroid command)
├── mcp_client/     # MCP protocol client
├── compose/        # Compose file parser, validator, roster generator
├── delegation/     # Delegation Board (task queue, state machine, dispute arbitration)
├── sandbox/        # OS-native sandbox (Linux: bwrap, macOS: sandbox-exec)
│   └── src/ lib.rs, config.rs, mandatory.rs, linux.rs, macos.rs, proxy/
├── runtime/        # Agent runtime (control loop, delegation processing, agent lifecycle)
│   └── src/
│       ├── lib.rs       # WorkspaceRuntime — spawn/manage all agents
│       ├── agent.rs     # AgentRuntime — single-agent control loop
│       └── engine.rs    # Engine — process() drives LLM + tool calls
├── memory/         # Memory system (Phase 3)
│   └── src/
│       ├── lib.rs       # Worklog + PersonalMemory trait
│       ├── worklog.rs   # Team worklog
│       └── personal.rs  # Personal memory storage and retrieval
└── plugins/        # Plugin system skeleton
```

### 6.2 Data Flow

```
User ──(PRIORITY_USER)──→ Delegation Board
                              │
   Manager ←── poll ──────────┘
        │
        ├── decompose ──→ Delegation Board
        │                    ├──→ Agent A (engine process)
        │                    │      │
        │                    │      ├── Need B → Delegation Board → Agent B
        │                    │      ├── Need recall → recall skill → Board(skip_worklog) → B searches memory.db → summary
        │                    │      └── Done → write worklog + personal memory → Board → Manager → Board → User
        │                    │
        │                    └──→ Agent C (parallel)
        │                           └── Done → write worklog + personal memory → Board → Manager
        │
        └── cancel() → Delegation Board (interrupt current task when user switches direction)
```

### 6.3 Provider Architecture

Upper-layer code depends only on the `AiProvider` trait, not on any concrete implementation. Adding a new provider (Anthropic, Groq, local models) requires only implementing the trait, with no changes to existing code.

```rust
pub trait AiProvider: Send + Sync {
    fn id(&self) -> &'static str;
    fn chat(&self, req: ChatRequest) -> impl Future<Output = Result<ChatResponse, ProviderError>>;
    fn chat_stream(&self, req: ChatRequest) -> impl Future<...>;
}
```

## 7. API Endpoints

| Method | Path          | Description                       |
| ------ | ------------- | --------------------------------- |
| GET    | /health       | Health check                      |
| GET    | /v1/providers | Query available AI provider list  |
| POST   | /v1/chat      | Non-streaming Chat request        |
| POST   | /v1/stream    | SSE streaming Chat request        |
| POST   | /admin/reload | Hot-reload provider configuration |

Planned:

| Method | Path                           | Description                   |
| ------ | ------------------------------ | ----------------------------- |
| POST   | /v1/workspace/create           | Create a Workspace            |
| GET    | /v1/workspace/{id}             | Query Workspace status        |
| POST   | /v1/workspace/{id}/task        | Submit a task to a Workspace  |
| GET    | /v1/workspace/{id}/delegations | Query Delegation Board status |

## 8. Implementation Roadmap

### Phase 1: Foundation
- [x] Basic crate structure (types, paths, assets, providers, server)
- [x] OpenRouter provider adaptation (streaming + non-streaming)
- [x] HTTP API framework (/v1/chat, /v1/stream, /v1/providers, /admin/reload)
- [x] Bridge remote control protocol
- [x] Project renamed from AliceCode to Margatroid

### Phase 2: Compose & Workspace
- [x] Compose file parser (parser + validator + roster generator)
- [x] Workspace lifecycle management (create, list, destroy)
- [x] OS-native sandbox (bwrap / sandbox-exec, guard, HTTP proxy, mandatory deny paths)
- [x] Delegation Board (priority queue, state machine, dispute arbitration, cancel, PRIORITY_USER)
- [x] Agent runtime (WorkspaceRuntime + AgentRuntime + engine control loop)
- [x] CLI (serve, compose validate/roster/load, workspace create/list)

### Phase 3: Memory & Recall (current)
- [ ] Worklog — team-shared summary entries, ~30 tokens each
- [ ] Personal Memory — per-agent independent storage, in-memory first
- [ ] Recall Skill — cross-agent memory queries via Board skip_worklog routing
- [ ] Engine tool-call loop — parse LLM tool calls and drive sandbox execution
- [ ] Agent context builder — fixed prefix (roster + worklog) + role prompt + delegation detail

### Phase 4: Production Readiness
- [ ] SQLite + FTS5 full-text search (replace in-memory implementation)
- [ ] Context compression — Memory Flush mechanism
- [ ] Full test coverage (unit, integration, end-to-end)
- [ ] CLI interaction polish
- [ ] Documentation and example compose files

## 9. Design Decisions Log

1. **Project naming** — Margatroid, from the Touhou Project character Alice Margatroid (the Seven-Colored Puppeteer). The original name AliceCode became unsuitable after the project evolved from a personal coding assistant into a multi-agent orchestration framework. CLI command is `margatroid`, recommended alias `mgt`.
2. **TOML over YAML** — Consistent with Rust ecosystem configuration conventions, and Margatroid uses TOML throughout for configuration.
3. **Event-driven over polling** — Avoids Delegation Board idle spinning, reducing resource consumption.
4. **Two-layer member discovery** — Roster (lightweight) + Profile (on-demand), controlling context window bloat.
5. **Worklog + Personal Memory dual-layer model** — The worklog is a team-shared fixed prefix (~30 tokens per entry); everyone is always aware of what others are doing. Personal memory is a private appendix, queried on demand. Recall is routed through the Delegation Board without writing to the worklog.
6. **Recall as a skill, not a task type** — Every agent has the recall skill by default, backed by the Board's `skip_worklog` channel. AgentRuntime does not need to distinguish task types.
7. **Manager is also an agent** — Not a hard-coded scheduler; maintains system flexibility.
8. **depends_on is self-documenting only** — Does not enforce startup ordering; collaboration relationships are dynamically established at runtime.
9. **OS-native sandbox over Docker/MicroVM** — Uses bubblewrap (Linux) and sandbox-exec (macOS) for process-level isolation. Agents inherit the host toolchain directly. Sandbox environments are ephemeral and disposable; only workdir and memory.db persist.
10. **Linux and macOS only** — Windows is not supported. No conditional compilation or compatibility layers.

## 10. Sandbox Reference Design & Tech Stack

### 10.1 Reference Projects

Margatroid's sandbox architecture is directly inspired by Anthropic's open-source `sandbox-runtime` (the sandbox runtime behind Claude Code), combined with existing Rust ecosystem crates.

| Reference Project | Language | What We Learn From It |
|---|---|---|
| `@anthropic-ai/sandbox-runtime` | TypeScript | Core architecture: SandboxManager, dual-proxy model, mandatory deny paths, config format |
| `astrid-workspace` | Rust | macOS Seatbelt profile dynamic generation, path injection safety validation, `sandbox-exec` argument construction |
| `extrasafe` | Rust | Linux seccomp BPF filter generation (optional defense-in-depth) |

### 10.2 Dependencies

| Crate | Purpose |
|---|---|
| `tokio` | Async runtime, process management, proxy server I/O |
| `serde` / `serde_json` | Sandbox configuration serialization |
| `tracing` | Logging and violation event tracking |
| `tempfile` | macOS Seatbelt configuration temporary files |
| `which` | Detect availability of `bwrap` / `sandbox-exec` |
| `hyper` + `http` | HTTP/HTTPS proxy server implementation |
| `tokio-socks` | SOCKS5 proxy implementation |
| `hickory-resolver` | DNS resolution (domain allowlist verification) |

### 10.3 Architecture Highlights

**Unified trait interface:**

```rust
pub trait Sandbox: Send + Sync {
    /// Initialize the sandbox (start proxy servers, etc.)
    async fn initialize(&mut self, config: SandboxConfig) -> Result<()>;

    /// Wrap an arbitrary shell command for sandboxed execution
    fn wrap_command(&self, cmd: &str) -> String;

    /// Reset the sandbox (stop proxies, clean up temp files)
    async fn reset(&mut self) -> Result<()>;
}
```

**Platform implementations:**
- `LinuxSandbox` — Invokes `bwrap`, configuring `--unshare-all`, `--bind` mounts, `--seccomp` BPF filters
- `MacOSSandbox` — Invokes `/usr/bin/sandbox-exec`, dynamically generating Seatbelt profiles (SBPL format)

**Defense-in-depth layers:**
- Base layer: namespace isolation (bwrap) or Seatbelt policy (macOS)
- Enhanced layer: seccomp BPF syscall filtering (Linux, optional)
- Network layer: HTTP/SOCKS5 proxy domain allowlisting
- Code layer: mandatory deny paths hard-coded, not configurable

### 10.4 Five Design Constraints Borrowed from Claude Code

1. **Write-denied-by-default, network-denied-by-default** — allow-only model, principle of least privilege
2. **Mandatory deny paths** — `~/.ssh`, `~/.aws`, `.env`, `.gitconfig`, `.git/hooks/`, `.mcp.json` hard-coded as non-writable
3. **Dual-proxy network filtering** — HTTP proxy for web traffic, SOCKS5 proxy for other TCP
4. **No toolchain bundling** — agents inherit host system tools directly (rustc, node, python, etc.)
5. **Anti-bypass** — `allowUnsandboxedCommands: false` prevents agents from disabling the sandbox
