# Workspace core

This crate owns the Workspace component, lifecycle events and the SQLite Agent registry. Its only project dependency is `core_plugin` (mecs). It does not own a Lua executor, load YAML, import another business core, or install Systems.

`WorkspaceDatabase::create(path, WorkspaceMetadata)` exclusively creates `workspace.sqlite` in an existing managed directory. The plugin first creates `agents/normal`, `agents/temporary` and `agents/archived`. `open(path)` opens only an existing v1 database, validates its schema and rows, resumes persisted archive intents, and exposes a confirmed initial snapshot. It never migrates an earlier layout.

Agent creation uses:

```rust,ignore
let operation = std::sync::Arc::new(database.try_operation(node_name)?);
let candidate = database.prepare_agent(
    &operation, parent_node, Some(node_name.into()), alias, tool_path, arguments,
)?;
let directory = database.agent_directory(&candidate)?;
// The plugin prepares files and opens AgentStorage in its blocking worker.
let record = database.commit_agent(&operation, candidate)?;
// Keep operation alive through StartAgent's original reply and child binding.
```

For temporary Agents call `temporary_operation_key()`, admit that key and pass `node_name=None`. The key includes its fresh persistent ID and becomes `runtime_name`; each invocation gets an independent registry record and directory. All normal node names are unique across the Workspace, while `parent_node` preserves the actual creator's complete node path. Reuse requires the same Driver path and parent. Omitting an alias preserves an existing value.

A `WorkspaceOperation` belongs to a specific shared database connection. It is intentionally not `Clone`; use `Arc` to retain it across workers and cancellation compensation. The last drop releases the node and wakes `wait_idle()`. `close_admission()` prevents new operations without invalidating already accepted work. `close()` returns Busy while any operation remains, explicitly closes the shared SQLite connection after idle, and sets the shared watch to `None`. Dropping one database clone does not close the connection.

`subscribe_changes()` supplies immutable, revisioned registry snapshots. Only confirmed transactions publish actual record changes; preparation, queries, rejected transactions and no-op reuse do not publish. This is a process-local commit source, not a SQLite file watcher.

`archive_agent(&operation, id)` is a synchronous blocking operation and assumes the plugin has already stopped the corresponding task and Agent. It persists an intent before moving the whole data directory. After a confirmed move it commits Archived, clears the current node name, preserves previous identity and alias, and publishes the committed snapshot. A failed commit leaves ArchivePending and a retryable intent. `recover_archives()` resumes the same recorded destination; ambiguous both-present/both-missing paths fail without overwriting data. Recovery does not start runtime entities or migrate user data.

Events retain the caller's original oneshot sender. `StartWorkspaceAgent<R,E>` has `Existing { workspace, node_name, reply }` and `New { workspace, tool_path, arguments, reply }`; parameter result types remain generic to avoid dependencies on tool core. `WorkspaceConfig` and `WorkspaceUpdateContext` are independent Resources. The component's `set_state` reports actual changes; the plugin publishes through the injected context after releasing its World component borrow.

`WorkspacePendingWork` is a separate Resource for startup and already accepted create work. `is_idle()` stays false before the startup load, while its token is in flight, and while any create token remains. The plugin transfers each token into a private completion marker after sending its StartWorkspace events; the existing startup System releases tokens after processing these events. This prevents application shutdown from overlooking an asynchronously loaded or newly created Workspace.

Validation:

```bash
cargo test -p workspace_core
cargo clippy -p workspace_core --all-targets -- -D warnings
```
