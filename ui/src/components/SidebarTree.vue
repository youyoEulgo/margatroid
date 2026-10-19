<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue';
import { Bot, ChevronRight, Ellipsis, FolderKanban, PenLine, Plus, Trash2 } from 'lucide-vue-next';
import type { SidebarModel, SidebarWorkspace } from '@/stores/workbench';

/**
 * Expandable workspace list: each workspace row opens to its agents plus a
 * "+ New session" row at the same level. Workspace and agent rows share one
 * row grammar; only workspace rows and agent rows carry the "..." menu.
 */
const props = defineProps<{
  model: SidebarModel;
  pending: boolean;
  offline: boolean;
}>();
const emit = defineEmits<{
  selectWorkspace: [key: string];
  toggleWorkspace: [key: string];
  selectAgent: [agent: string, workspaceKey: string];
  renameWorkspace: [key: string];
  deleteWorkspace: [key: string];
  renameAgent: [workspaceKey: string, agent: string];
  deleteAgent: [workspaceKey: string, agent: string];
  newSession: [workspaceKey: string];
  newWorkspace: [];
}>();

type MenuTarget = { kind: 'workspace'; key: string } | { kind: 'agent'; workspaceKey: string; agent: string };

const openMenu = ref<string | null>(null);
const menuRoot = ref<HTMLElement | null>(null);
const menuButtons = ref(new Map<string, HTMLButtonElement>());


async function toggleMenu(target: MenuTarget, event: MouseEvent) {
  const id = menuId(target);
  if (openMenu.value === id) {
    closeMenu();
    return;
  }
  const button = event.currentTarget instanceof HTMLElement ? event.currentTarget : null;
  if (button) menuButtons.value.set(id, button as HTMLButtonElement);
  openMenu.value = id;
  await nextTick();
  const item = menuRoot.value?.querySelector<HTMLElement>(`[data-menu-for="${CSS.escape(id)}"] [role="menuitem"]`);
  item?.focus();
}
function menuId(target: MenuTarget) {
  return target.kind === 'workspace' ? `workspace:${target.key}` : `agent:${target.workspaceKey}:${target.agent}`;
}
function closeMenu(restoreFocus = false) {
  const id = openMenu.value;
  openMenu.value = null;
  if (restoreFocus && id) menuButtons.value.get(id)?.focus();
  if (id) menuButtons.value.delete(id);
}
function choose(target: MenuTarget, action: 'rename' | 'delete') {
  closeMenu(true);
  if (target.kind === 'workspace') {
    if (action === 'rename') emit('renameWorkspace', target.key);
    else emit('deleteWorkspace', target.key);
  } else if (action === 'rename') emit('renameAgent', target.workspaceKey, target.agent);
  else emit('deleteAgent', target.workspaceKey, target.agent);
}
function workspaceMenuTarget(key: string): MenuTarget {
  return { kind: 'workspace', key };
}
function agentMenuTarget(workspaceKey: string, agent: string): MenuTarget {
  return { kind: 'agent', workspaceKey, agent };
}
function isMenuOpen(target: MenuTarget) {
  return openMenu.value === menuId(target);
}

function focusSibling(direction: number, event: KeyboardEvent) {
  const menu = event.currentTarget as HTMLElement | null;
  const items = [...(menu?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? [])];
  const index = items.indexOf(document.activeElement as HTMLElement);
  const next = items[(index + direction + items.length) % items.length];
  next?.focus();
}

function onDocumentPointer(event: MouseEvent) {
  if (!openMenu.value) return;
  const target = event.target;
  if (target instanceof Element && target.closest('[role=menu]')) return;
  const button = menuButtons.value.get(openMenu.value);
  if (target instanceof Node && button?.contains(target)) return;
  closeMenu();
}
function onDocumentKey(event: KeyboardEvent) {
  if (event.key !== 'Escape' || !openMenu.value) return;
  event.stopPropagation();
  closeMenu(true);
}
document.addEventListener('pointerdown', onDocumentPointer, true);
document.addEventListener('keydown', onDocumentKey);
onBeforeUnmount(() => {
  document.removeEventListener('pointerdown', onDocumentPointer, true);
  document.removeEventListener('keydown', onDocumentKey);
});

watch(
  () => props.model.workspaces.map((workspace) => `${workspace.key}:${workspace.expanded}:${workspace.agents.map((agent) => agent.id).join(',')}`).join('|'),
  () => closeMenu(),
);

const disabled = computed(() => props.pending || props.offline);
const statusLabel: Record<SidebarWorkspace['status'], string> = {
  stopped: 'Stopped',
  running: 'Running',
  error: 'Error',
};
function rowTitle(name: string, status: SidebarWorkspace['status'], reason: string) {
  return reason ? `${name} — ${statusLabel[status]}: ${reason}` : `${name} — ${statusLabel[status]}`;
}
</script>

<template>
  <div ref="menuRoot" class="sidebar-tree">
    <ul v-if="model.workspaces.length" class="tree-list" role="tree" aria-label="Workspaces">
      <li
        v-for="workspace in model.workspaces"
        :key="workspace.key"
        class="tree-node"
        role="treeitem"
        :aria-expanded="workspace.expanded"
        :aria-selected="workspace.active"
      >
        <div :class="['tree-row', 'tree-row--workspace', { 'tree-row--active': workspace.active }]">
          <button
            class="tree-disclosure"
            type="button"
            :aria-label="`${workspace.expanded ? 'Collapse' : 'Expand'} ${workspace.name}`"
            @click="emit('toggleWorkspace', workspace.key)"
          >
            <ChevronRight :class="['tree-caret', { 'is-open': workspace.expanded }]" :size="14" />
          </button>
          <button
            class="tree-main"
            type="button"
            :title="`${rowTitle(workspace.name, workspace.status, workspace.reason)} — ${workspace.projectRoot || workspace.path}`"
            :aria-label="rowTitle(workspace.name, workspace.status, workspace.reason)"
            @click="emit('toggleWorkspace', workspace.key)"
          >
            <span :class="['status-dot', `status-dot--${workspace.status}`]" aria-hidden="true"></span>
            <FolderKanban class="tree-icon" :size="15" />
            <span class="tree-name">{{ workspace.name }}</span>
            <span v-if="workspace.invalid" class="tree-flag" title="Invalid definition">invalid</span>
          </button>
          <div class="row-menu">
            <button
              class="row-action"
              type="button"
              :aria-label="`Workspace actions for ${workspace.name}`"
              aria-haspopup="menu"
              :aria-expanded="isMenuOpen(workspaceMenuTarget(workspace.key))"
              @click="toggleMenu(workspaceMenuTarget(workspace.key), $event)"
            >
              <Ellipsis :size="16" />
            </button>
            <div
              v-if="isMenuOpen(workspaceMenuTarget(workspace.key))"
              class="menu"
              role="menu"
              :data-menu-for="menuId(workspaceMenuTarget(workspace.key))"
              @keydown.down.prevent="focusSibling(1, $event)"
              @keydown.up.prevent="focusSibling(-1, $event)"
            >
              <button role="menuitem" type="button" class="menu-item" @click="choose(workspaceMenuTarget(workspace.key), 'rename')">
                <PenLine :size="14" /> Rename
              </button>
              <button role="menuitem" type="button" class="menu-item menu-item--danger" @click="choose(workspaceMenuTarget(workspace.key), 'delete')">
                <Trash2 :size="14" /> Delete
              </button>
            </div>
          </div>
        </div>

        <ul v-if="workspace.expanded" class="tree-children" role="group">
          <li
            v-for="agent in workspace.agents"
            :key="agent.id"
            :class="['tree-row', 'tree-row--agent', { 'tree-row--active': agent.active }]"
            role="treeitem"
            :aria-selected="agent.active"
          >
            <button
              class="tree-main tree-main--agent"
              type="button"
              :title="`${rowTitle(agent.name, agent.status, agent.reason)} — ${agent.driver}`"
              :aria-label="rowTitle(agent.name, agent.status, agent.reason)"
              @click="emit('selectAgent', agent.id, workspace.key)"
            >
              <span :class="['status-dot', `status-dot--${agent.status}`]" aria-hidden="true"></span>
              <Bot class="tree-icon" :size="15" />
              <span class="tree-name">{{ agent.name }}</span>
            </button>
            <div class="row-menu">
              <button
                class="row-action"
                type="button"
                :aria-label="`Agent actions for ${agent.name}`"
                aria-haspopup="menu"
                :aria-expanded="isMenuOpen(agentMenuTarget(workspace.key, agent.id))"
                @click="toggleMenu(agentMenuTarget(workspace.key, agent.id), $event)"
              >
                <Ellipsis :size="16" />
              </button>
              <div
                v-if="isMenuOpen(agentMenuTarget(workspace.key, agent.id))"
                class="menu"
                role="menu"
                :data-menu-for="menuId(agentMenuTarget(workspace.key, agent.id))"
                @keydown.down.prevent="focusSibling(1, $event)"
                @keydown.up.prevent="focusSibling(-1, $event)"
              >
                <button role="menuitem" type="button" class="menu-item" @click="choose(agentMenuTarget(workspace.key, agent.id), 'rename')">
                  <PenLine :size="14" /> Rename
                </button>
                <button role="menuitem" type="button" class="menu-item menu-item--danger" @click="choose(agentMenuTarget(workspace.key, agent.id), 'delete')">
                  <Trash2 :size="14" /> Delete
                </button>
              </div>
            </div>
          </li>
          <li>
            <button
              class="tree-row tree-row--add tree-row--session"
              type="button"
              :disabled="disabled || workspace.invalid"
              :title="workspace.invalid ? 'Fix the workspace definition before adding agents' : 'Create an agent in this workspace'"
              @click="emit('newSession', workspace.key)"
            >
              <Plus class="tree-icon" :size="15" />
              <span class="tree-name">New session</span>
            </button>
          </li>
        </ul>
      </li>
      <li>
        <button
          class="tree-row tree-row--add tree-row--workspace-add"
          type="button"
          :disabled="disabled"
          title="Create a workspace from a name and an absolute project path"
          @click="emit('newWorkspace')"
        >
          <Plus class="tree-icon" :size="15" />
          <span class="tree-name">New workspace</span>
        </button>
      </li>
    </ul>
    <div v-else class="sidebar-empty">
      <span>No workspaces registered yet.</span>
      <button class="tree-row tree-row--add" type="button" :disabled="disabled" @click="emit('newWorkspace')">
        <Plus class="tree-icon" :size="15" />
        <span class="tree-name">New workspace</span>
      </button>
    </div>
  </div>
</template>