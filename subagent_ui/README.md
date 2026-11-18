# Margatroid Subagent UI

Read-only subagent interface loaded by the platform shell through `agent.ui`.

This project is copied from the Driver UI and keeps the same message rendering,
history, status and Activity behavior. The composer and every bottom-bar
control are removed; there is no send, upload, model selector, tool manager,
compaction action or advanced editor.

- Shared protocol/store/components: `../ui/src` (aliased as `@`)
- Entry: `src/main.ts` and `src/DriverApp.vue`
- Build: `bun run build`
- Outputs:
  - `../resources/agent-drivers/local/coder/latest/subagent-ui/index.html`
  - `../resources/agent-drivers/local/reviewer/latest/subagent-ui/index.html`

The project reuses `../ui/node_modules` (created as a symlink by the build) so
Vue, Pinia and the shared modules resolve to one runtime copy. `driver_ui`
builds only the full Driver UI; this project builds only the two subagent UI
resources.
