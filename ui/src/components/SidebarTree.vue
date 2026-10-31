<script setup lang="ts">
import { Bot, ChevronRight, FolderKanban } from 'lucide-vue-next';
import type { SidebarModel } from '@/stores/workbench';

defineProps<{ model: SidebarModel; pending: boolean; offline: boolean }>();
const emit = defineEmits<{
  selectWorkspace: [path: string];
  toggleWorkspace: [path: string];
  selectAgent: [agent: string, path: string];
}>();
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
        <div :class="['tree-row', 'tree-row--workspace', { 'tree-row--active': workspace.active }]">
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
            <span
              :class="['status-dot', `status-dot--${workspace.status}`]"
              aria-hidden="true"
            ></span>
            <FolderKanban class="tree-icon" :size="15" />
            <span class="tree-name">{{ workspace.name }}</span>
          </button>
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
