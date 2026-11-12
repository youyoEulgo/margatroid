# Margatroid Driver UI

Central agent interface loaded by the platform shell through `agent.ui`.

- Shared protocol/store/components: `../ui/src` (aliased as `@`)
- Entry: `src/main.ts` and `src/DriverApp.vue`
- Build: `bun run build`
- Output: `../resources/agent-drivers/local/coder/latest/ui/index.html`

The project reuses `../ui/node_modules` (created as a symlink by the build) so
Vue, Pinia and the shared modules resolve to one runtime copy.
