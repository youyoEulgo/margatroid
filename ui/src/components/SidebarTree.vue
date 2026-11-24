<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from 'vue';
import {
  Archive,
  Bot,
  ChevronRight,
  CornerDownRight,
  Ellipsis,
  FolderKanban,
  Pause,
  Pencil,
  Play,
  Trash2,
  Plus,
} from 'lucide-vue-next';
import type { SidebarAgent, SidebarModel, SidebarWorkspace } from '@/stores/workbench';

defineProps<{ model: SidebarModel; pending: boolean; offline: boolean }>();
const emit = defineEmits<{
  selectWorkspace: [path: string];
  toggleWorkspace: [path: string];
  selectAgent: [agent: string, path: string];
  addAgent: [workspace: string];
  renameWorkspace: [address: string, alias: string];
  deleteWorkspace: [address: string];
  startAgent: [workspace: string, nodeName: string];
  stopAgent: [address: string];
  renameAgent: [address: string, alias: string];
  archiveAgent: [workspace: string, nodeName: string];
}>();

const menuAddress = ref('');
const workspaceMode = ref<'menu' | 'rename' | 'delete'>('menu');
const workspaceAlias = ref('');
const agentMenuKey = ref('');
const agentMode = ref<'menu' | 'rename'>('menu');
const agentAlias = ref('');

function closeMenus() {
  menuAddress.value = '';
  agentMenuKey.value = '';
}
function toggleMenu(workspace: SidebarWorkspace) {
  if (menuAddress.value === workspace.address) {
    menuAddress.value = '';
    return;
  }
  menuAddress.value = workspace.address;
  workspaceMode.value = 'menu';
  workspaceAlias.value = workspace.name;
  agentMenuKey.value = '';
}
function openRename(workspace: SidebarWorkspace) {
  workspaceAlias.value = workspace.name;
  workspaceMode.value = 'rename';
}
function saveRename(workspace: SidebarWorkspace) {
  const alias = workspaceAlias.value.trim();
  if (!alias) return;
  emit('renameWorkspace', workspace.address, alias);
  closeMenus();
}
function confirmDelete(workspace: SidebarWorkspace) {
  emit('deleteWorkspace', workspace.address);
  closeMenus();
}
function toggleAgentMenu(workspace: SidebarWorkspace, agent: SidebarAgent) {
  if (agentMenuKey.value === agent.key) {
    agentMenuKey.value = '';
    return;
  }
  agentMenuKey.value = agent.key;
  agentMode.value = 'menu';
  agentAlias.value = agent.name;
  menuAddress.value = '';
}
function saveAgentAlias(agent: SidebarAgent) {
  const alias = agentAlias.value.trim();
  if (!alias || !agent.id) return;
  emit('renameAgent', agent.id, alias);
  closeMenus();
}
function toggleAgentRun(workspace: SidebarWorkspace, agent: SidebarAgent) {
  if (agent.running && agent.id) emit('stopAgent', agent.id);
  else if (agent.nodeName) emit('startAgent', workspace.address, agent.nodeName);
}
function archiveAgent(workspace: SidebarWorkspace, agent: SidebarAgent) {
  if (!agent.nodeName) return;
  emit('archiveAgent', workspace.address, agent.nodeName);
  closeMenus();
}
onMounted(() => document.addEventListener('click', closeMenus));
onBeforeUnmount(() => document.removeEventListener('click', closeMenus));
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
            @click="emit('toggleWorkspace', workspace.path)"
          >
            <ChevronRight :class="['tree-caret', { 'is-open': workspace.expanded }]" :size="14" />
          </button>
          <button
            class="tree-main"
            type="button"
            :title="workspace.address"
            @click="emit('toggleWorkspace', workspace.path)"
          >
            <FolderKanban class="tree-icon" :size="15" />
            <span class="tree-name">{{ workspace.name }}</span>
          </button>
          <div class="tree-actions workspace-actions" @click.stop>
            <button
              class="tree-action"
              type="button"
              title="Add Agent"
              :disabled="pending || offline"
              @click="emit('addAgent', workspace.address)"
            >
              <Plus :size="14" />
            </button>
            <button
              class="tree-action"
              type="button"
              title="Workspace actions"
              :disabled="pending || offline"
              @click="toggleMenu(workspace)"
            >
              <Ellipsis :size="15" />
            </button>
            <div v-if="menuAddress === workspace.address" class="workspace-action-menu" @click.stop>
              <template v-if="workspaceMode === 'menu'">
                <button type="button" @click="openRename(workspace)">
                  <Pencil :size="13" /> Rename alias
                </button>
                <button
                  type="button"
                  class="workspace-action-menu-danger"
                  @click="workspaceMode = 'delete'"
                >
                  <Trash2 :size="13" /> Delete workspace
                </button>
              </template>
              <form v-else-if="workspaceMode === 'rename'" @submit.prevent="saveRename(workspace)">
                <input v-model="workspaceAlias" class="field-input" placeholder="Workspace alias" />
                <div>
                  <button type="button" @click="workspaceMode = 'menu'">Cancel</button
                  ><button type="submit">Save</button>
                </div>
              </form>
              <template v-else>
                <p>Delete this workspace and all managed Agent data?</p>
                <div>
                  <button type="button" @click="workspaceMode = 'menu'">Cancel</button
                  ><button
                    type="button"
                    class="workspace-action-menu-danger"
                    @click="confirmDelete(workspace)"
                  >
                    Delete
                  </button>
                </div>
              </template>
            </div>
          </div>
        </div>
        <ul v-if="workspace.expanded" class="tree-children" role="group">
          <li
            v-for="agent in workspace.agents"
            :key="agent.key"
            :class="[
              'tree-row',
              'tree-row--agent',
              { 'tree-row--active': agent.active, 'tree-row--child': agent.depth > 0 },
            ]"
            role="treeitem"
            :aria-level="agent.depth + 1"
            :aria-selected="agent.active"
            :style="{ paddingLeft: `${8 + agent.depth * 18}px` }"
          >
            <button
              class="tree-main tree-main--agent"
              type="button"
              :disabled="!agent.id"
              :title="
                [
                  agent.id || `${agent.name}: not running`,
                  agent.category ?? '',
                  agent.creator?.task_id ? `task ${agent.creator.task_id}` : '',
                ]
                  .filter(Boolean)
                  .join(' · ')
              "
              @click="agent.id && emit('selectAgent', agent.id, workspace.path)"
            >
              <span
                :class="['status-dot', `status-dot--${agent.status}`]"
                aria-hidden="true"
              ></span>
              <CornerDownRight v-if="agent.depth > 0" class="tree-branch" :size="12" />
              <Bot class="tree-icon" :size="15" />
              <span class="tree-name">{{ agent.name }}</span>
              <span v-if="agent.category === 'archived'" class="tree-flag">archived</span>
              <span v-else-if="agent.category === 'temporary'" class="tree-flag">temporary</span>
            </button>
            <div class="tree-actions agent-actions" @click.stop>
              <button
                v-if="agent.category !== 'archived'"
                class="tree-action"
                type="button"
                :title="agent.running ? 'Stop agent' : 'Start agent'"
                :disabled="pending || offline || (!agent.running && !agent.nodeName)"
                @click="toggleAgentRun(workspace, agent)"
              >
                <Pause v-if="agent.running" :size="13" />
                <Play v-else :size="13" />
              </button>
              <button
                class="tree-action"
                type="button"
                title="Agent actions"
                :disabled="pending || offline"
                @click="toggleAgentMenu(workspace, agent)"
              >
                <Ellipsis :size="14" />
              </button>
              <div
                v-if="agentMenuKey === agent.key"
                class="workspace-action-menu agent-action-menu"
                @click.stop
              >
                <template v-if="agentMode === 'menu'">
                  <button type="button" @click="agentMode = 'rename'">
                    <Pencil :size="13" /> Rename alias
                  </button>
                  <button
                    v-if="agent.category === 'normal' && agent.nodeName"
                    type="button"
                    class="workspace-action-menu-danger"
                    @click="archiveAgent(workspace, agent)"
                  >
                    <Archive :size="13" /> Archive agent
                  </button>
                </template>
                <form v-else @submit.prevent="saveAgentAlias(agent)">
                  <input v-model="agentAlias" class="field-input" placeholder="Agent alias" />
                  <div>
                    <button type="button" @click="agentMode = 'menu'">Cancel</button
                    ><button type="submit">Save</button>
                  </div>
                </form>
              </div>
            </div>
          </li>
          <li v-if="!workspace.agents.length" class="sidebar-footnote">No agents registered.</li>
        </ul>
      </li>
    </ul>
    <div v-else class="sidebar-empty"><span>No workspaces loaded.</span></div>
  </div>
</template>
