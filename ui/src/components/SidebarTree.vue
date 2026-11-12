<script setup lang="ts">
import { ref, watch } from 'vue';
import {
  Bot,
  ChevronRight,
  FolderKanban,
  LoaderCircle,
  Pause,
  Play,
  RotateCw,
} from 'lucide-vue-next';
import type { SidebarModel, SidebarWorkspace } from '@/stores/workbench';

const props = defineProps<{ model: SidebarModel; pending: boolean; offline: boolean }>();
const busyKey = ref('');
watch(
  () => props.pending,
  (pending) => {
    if (!pending) busyKey.value = '';
  },
);
function begin(key: string, action: () => void) {
  busyKey.value = key;
  action();
  queueMicrotask(() => {
    if (!props.pending) busyKey.value = '';
  });
}
function isBusy(workspace: SidebarWorkspace): boolean {
  return (
    busyKey.value === workspace.key ||
    ['starting', 'restarting', 'stopping'].includes(workspace.lifecycle)
  );
}
const emit = defineEmits<{
  selectWorkspace: [path: string];
  toggleWorkspace: [path: string];
  selectAgent: [agent: string, path: string];
  startWorkspace: [path: string];
  restartWorkspace: [path: string];
  stopWorkspace: [address: string];
}>();

function startWorkspace(workspace: SidebarWorkspace) {
  begin(workspace.key, () => emit('startWorkspace', workspace.path));
}
function restartWorkspace(workspace: SidebarWorkspace) {
  begin(workspace.key, () => emit('restartWorkspace', workspace.path));
}
function stopWorkspace(workspace: SidebarWorkspace) {
  if (workspace.address)
    begin(workspace.key, () => emit('stopWorkspace', workspace.address!));
}
</script>

<template>
  <div class="sidebar-tree">
    <ul v-if="model.workspaces.length" class="tree-list" role="tree" aria-label="Workspaces">
      <li
        v-for="workspace in model.workspaces"
        :key="workspace.key"
        class="tree-node"
        role="treeitem"
        :aria-expanded="workspace.expanded"
        :aria-selected="workspace.active"
      >
        <div class="tree-row tree-row--workspace">
          <button
            class="tree-disclosure"
            type="button"
            :aria-label="`${workspace.expanded ? 'Collapse' : 'Expand'} ${workspace.name}`"
            @click="emit('toggleWorkspace', workspace.path)"
          >
            <ChevronRight :class="['tree-caret', { 'is-open': workspace.expanded }]" :size="14" />
          </button>
          <button
            class="tree-main"
            type="button"
            :title="`${workspace.path} · ${workspace.lifecycle}${workspace.reason ? `: ${workspace.reason}` : ''}`"
            @click="emit('toggleWorkspace', workspace.path)"
          >
            <FolderKanban class="tree-icon" :size="15" />
            <span class="tree-name">{{ workspace.name }}</span>
          </button>
          <div class="tree-actions">
            <span
              v-if="isBusy(workspace)"
              class="tree-action tree-action--spinner"
              :title="`${workspace.name} is ${workspace.lifecycle}`"
              aria-label="Workspace operation in progress"
            >
              <LoaderCircle class="spin" :size="14" />
            </span>
            <template v-else-if="workspace.lifecycle === 'running'">
              <button
                class="tree-action"
                type="button"
                title="Restart workspace"
                :disabled="pending || offline"
                @click.stop="restartWorkspace(workspace)"
              >
                <RotateCw :size="14" />
              </button>
              <button
                class="tree-action"
                type="button"
                title="Stop workspace"
                :disabled="pending || offline"
                @click.stop="stopWorkspace(workspace)"
              >
                <Pause :size="14" />
              </button>
            </template>
            <button
              v-else-if="workspace.lifecycle === 'stopped'"
              class="tree-action"
              type="button"
              title="Start workspace"
              :disabled="pending || offline"
              @click.stop="startWorkspace(workspace)"
            >
              <Play :size="14" />
            </button>
          </div>
        </div>
        <ul v-if="workspace.expanded" class="tree-children" role="group">
          <li
            v-for="agent in workspace.agents"
            :key="agent.key"
            :class="['tree-row', 'tree-row--agent', { 'tree-row--active': agent.active }]"
            role="treeitem"
            :aria-selected="agent.active"
          >
            <button
              class="tree-main tree-main--agent"
              type="button"
              :disabled="!agent.id"
              :title="agent.reason || agent.id || `${agent.name}: workspace is not running`"
              @click="agent.id && emit('selectAgent', agent.id, workspace.path)"
            >
              <span
                :class="['status-dot', `status-dot--${agent.status}`]"
                aria-hidden="true"
              ></span>
              <Bot class="tree-icon" :size="15" />
              <span class="tree-name">{{ agent.name }}</span>
            </button>
          </li>
          <li v-if="!workspace.agents.length" class="sidebar-footnote">No members configured.</li>
        </ul>
      </li>
    </ul>
    <div v-else class="sidebar-empty"><span>No workspace definitions loaded.</span></div>
  </div>
</template>
