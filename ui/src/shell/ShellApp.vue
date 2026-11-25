<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { storeToRefs } from 'pinia';
import {
  Bot,
  Boxes,
  Command,
  MessageSquare,
  PanelLeft,
  PanelRight,
  Plus,
  Settings2,
  UsersRound,
  X,
} from 'lucide-vue-next';
import SidebarTree from '@/components/SidebarTree.vue';
import FormSelect from '@/components/FormSelect.vue';
import ModelConfigPanel from '@/components/ModelConfigPanel.vue';
import { useWorkbenchStore } from '@/stores/workbench';
import { buildDriverDocument } from '@/lib/driverUi';
import InfoPanel from './InfoPanel.vue';
import type { ResourceItem } from '@/types/protocol';
import { formatRecordTime } from '@/lib/time';

const store = useWorkbenchStore();
const {
  endpoint,
  connectionStatus,
  connectionError,
  workspaces,
  selectedWorkspace,
  selectedAgent,
  selectedAgentState,
  selectedAgentReady,
  sidebarTree,
  workspaceOperationPending,
  workspaceNotice,
  workspaceSnapshots,
  agentUiBundle,
  agentUiLoading,
  agentLogHistoryLoaded,
  selectedSessionLogs,
  selectedMclEntries,
} = storeToRefs(store);

const sidebarOpen = ref(false);
const sidebarCollapsed = ref(false);
const viewMode = ref<'driver' | 'info'>(
  typeof window !== 'undefined' && window.location.hash === '#info' ? 'info' : 'driver',
);
const activityOpen = ref(false);
const activityTab = ref<'activity' | 'mcl'>('activity');
const logFilter = ref<'all' | 'errors'>('all');
const settingsOpen = ref(false);
const settingsTab = ref<'connection' | 'models'>('connection');
const endpointDraft = ref(endpoint.value);
const formError = ref('');
const driverDocument = ref('');
const driverDocumentError = ref('');
const iframeKey = ref('');
const createOpen = ref(false);
const createName = ref('');
const createAlias = ref('');
const createRoot = ref('');
const createBusy = ref(false);
const createError = ref('');
const createAdvanced = ref(false);
const createNameTouched = ref(false);
const createAliasTouched = ref(false);
const resources = ref<ResourceItem[]>([]);
const newAgentNode = ref('');
const newAgentAlias = ref('');
const newAgentPath = ref('.');
const newAgentTool = ref('');
const addAgentOpen = ref(false);
const addAgentWorkspace = ref('');
const addAgentAdvanced = ref(false);
const addAgentBusy = ref(false);
const addAgentError = ref('');
const addAgentTemporary = ref(false);
const addAgentNodeTouched = ref(false);
const addAgentAliasTouched = ref(false);
const driverOptions = computed(() =>
  resources.value
    .filter((resource) => resource.tool_path)
    .map((resource) => ({ value: resource.tool_path!, label: resource.address })),
);

const connectionLabel = computed(() => {
  if (connectionStatus.value === 'online') return 'Connected';
  if (connectionStatus.value === 'connecting') return 'Connecting';
  return 'Offline';
});
const activityLogs = computed(() => selectedSessionLogs.value);
const displayedLogs = computed(() =>
  logFilter.value === 'errors'
    ? activityLogs.value.filter((entry) => entry.level === 'ERROR' || entry.level === 'WARN')
    : activityLogs.value,
);
const channelLabel = computed(() => {
  if (selectedAgent.value)
    return selectedAgent.value.split('/').pop()?.split(':')[0] ?? selectedAgent.value;
  if (selectedWorkspace.value) return selectedWorkspace.value.name;
  return 'No agent selected';
});
const uiState = computed(() => {
  if (!selectedAgent.value) return 'Choose an agent to load its Driver UI.';
  if (agentUiLoading.value) return 'Loading Driver UI…';
  if (!agentUiBundle.value) return 'No Driver UI.';
  return '';
});
function renderDriver() {
  driverDocumentError.value = '';
  if (!agentUiBundle.value || !selectedAgent.value) {
    driverDocument.value = '';
    return;
  }
  try {
    driverDocument.value = buildDriverDocument(agentUiBundle.value, {
      endpoint: endpoint.value,
      address: selectedAgent.value,
    });
    iframeKey.value = `${selectedAgent.value}:${agentUiBundle.value.revision}`;
  } catch {
    driverDocumentError.value = 'Driver UI failed to load';
    driverDocument.value = '';
  }
}
watch([agentUiBundle, selectedAgent, endpoint], renderDriver, {
  immediate: true,
});
watch([activityOpen, activityTab, selectedAgent], () => {
  if (activityOpen.value && selectedAgentReady.value && activityTab.value === 'mcl')
    store.queryRecords('mcl');
});
watch(viewMode, (mode) => {
  const hash = mode === 'info' ? '#info' : '#driver';
  if (window.location.hash !== hash) history.replaceState(null, '', hash);
});
function toggleSidebar() {
  if (window.innerWidth <= 760) {
    sidebarOpen.value = !sidebarOpen.value;
    if (sidebarOpen.value) sidebarCollapsed.value = false;
    return;
  }
  sidebarCollapsed.value = !sidebarCollapsed.value;
  if (sidebarCollapsed.value) sidebarOpen.value = false;
}
function selectWorkspace(path: string) {
  store.selectWorkspace(path);
  sidebarOpen.value = false;
}
function selectMember(agent: string | null, path?: string) {
  store.selectAgent(agent, path);
  sidebarOpen.value = false;
}
function directoryBaseName(value: string): string {
  const base = value.trim().split(/[\\/]/u).filter(Boolean).pop() ?? '';
  return base.replace(/[\\/:\x00-\x1f\x7f]+/gu, '-').trim() || 'workspace';
}
function uniqueWorkspaceNode(base: string): string {
  const used = new Set(
    workspaceSnapshots.value.map(
      (workspace) => workspace.address.split('/').filter(Boolean).pop() ?? '',
    ),
  );
  if (!used.has(base)) return base;
  let index = 2;
  while (used.has(`${base}-${index}`)) index += 1;
  return `${base}-${index}`;
}
function refreshCreateDefaults() {
  if (!createRoot.value.trim()) return;
  const base = directoryBaseName(createRoot.value);
  if (!createNameTouched.value) createName.value = uniqueWorkspaceNode(base);
  if (!createAliasTouched.value) createAlias.value = createName.value;
}
function openCreateWorkspace() {
  createName.value = '';
  createAlias.value = '';
  createRoot.value = '';
  createError.value = '';
  createAdvanced.value = false;
  createNameTouched.value = false;
  createAliasTouched.value = false;
  createOpen.value = true;
}
async function createManagedWorkspace() {
  if (createBusy.value) return;
  refreshCreateDefaults();
  if (!createName.value.trim()) {
    createError.value = 'Node name is required';
    return;
  }
  if (createName.value.includes('/') || createName.value.includes('\\')) {
    createError.value = 'Node name must be a single path segment';
    return;
  }
  if (!createRoot.value.trim().startsWith('/')) {
    createError.value = 'Workspace directory must be an absolute server path';
    return;
  }
  createBusy.value = true;
  createError.value = '';
  try {
    await store.createWorkspace({
      address: `/${createName.value.trim()}`,
      alias: createAlias.value.trim() || createName.value.trim(),
      project_root: createRoot.value.trim(),
    });
    createOpen.value = false;
  } catch (error) {
    createError.value = error instanceof Error ? error.message : 'Create failed';
  } finally {
    createBusy.value = false;
  }
}
async function loadResources() {
  try {
    const result = (await store.queryResources({ type: 'driver' })) as ResourceItem[] | null;
    resources.value = Array.isArray(result) ? result : [];
    if (!newAgentTool.value && resources.value[0]?.tool_path)
      newAgentTool.value = resources.value[0].tool_path;
  } catch {
    resources.value = [];
  }
}
function startSidebarAgent(workspace: string, nodeName: string) {
  void store.startAgentExisting(workspace, nodeName).catch(() => {
    workspaceNotice.value = 'Agent start failed';
  });
}
function stopSidebarAgent(address: string) {
  void store.stopAgent(address).catch(() => {
    workspaceNotice.value = 'Agent stop failed';
  });
}
function renameSidebarAgent(workspace: string, agentId: string, alias: string | null) {
  void store.renameAgentAlias(workspace, agentId, alias).catch(() => {
    workspaceNotice.value = 'Agent rename failed';
  });
}
function archiveSidebarAgent(workspace: string, nodeName: string) {
  void store.archiveAgent(workspace, nodeName).catch(() => {
    workspaceNotice.value = 'Agent archive failed';
  });
}
function renameWorkspaceAlias(address: string, alias: string) {
  void store.renameWorkspaceAlias(address, alias).catch(() => {
    workspaceNotice.value = 'Workspace rename failed';
  });
}
function deleteWorkspace(address: string) {
  void store.deleteWorkspace(address).catch(() => {
    workspaceNotice.value = 'Workspace delete failed';
  });
}
function uniqueNodeForWorkspace(workspace: string, base: string): string {
  const snapshot = workspaceSnapshots.value.find((item) => item.address === workspace);
  const used = new Set(
    (snapshot?.agents ?? [])
      .filter((agent) => agent.category !== 'archived')
      .map((agent) => agent.node_name)
      .filter((name): name is string => typeof name === 'string'),
  );
  if (!used.has(base)) return base;
  let index = 2;
  while (used.has(`${base}-${index}`)) index += 1;
  return `${base}-${index}`;
}
function openAgentDialog(workspaceAddress: string) {
  addAgentWorkspace.value = workspaceAddress;
  addAgentOpen.value = true;
  addAgentAdvanced.value = false;
  addAgentBusy.value = false;
  addAgentError.value = '';
  addAgentTemporary.value = false;
  addAgentNodeTouched.value = false;
  addAgentAliasTouched.value = false;
  newAgentNode.value = '';
  newAgentAlias.value = '';
  newAgentPath.value = '.';
  newAgentTool.value = '';
  void loadResources().then(() => applyAgentDefaults());
}
function applyAgentDefaults() {
  const resource = resources.value.find((item) => item.tool_path === newAgentTool.value);
  if (!resource) return;
  const base = resource.name;
  if (!addAgentNodeTouched.value)
    newAgentNode.value = uniqueNodeForWorkspace(addAgentWorkspace.value, base);
  if (!addAgentAliasTouched.value) newAgentAlias.value = newAgentNode.value;
}
async function startNewAgentFromDialog() {
  if (!newAgentTool.value || addAgentBusy.value) return;
  addAgentBusy.value = true;
  addAgentError.value = '';
  try {
    const args: Record<string, unknown> = {
      path: newAgentPath.value.trim() || '.',
      ...(newAgentAlias.value.trim() ? { alias: newAgentAlias.value.trim() } : {}),
    };
    if (!addAgentTemporary.value) args.node_name = newAgentNode.value.trim();
    await store.startAgentNew(addAgentWorkspace.value, newAgentTool.value, args);
    addAgentOpen.value = false;
  } catch (error) {
    addAgentError.value = error instanceof Error ? error.message : 'Agent start failed';
  } finally {
    addAgentBusy.value = false;
  }
}
function closeOverlaysOnEscape(event: KeyboardEvent) {
  if (event.key !== 'Escape') return;
  if (createOpen.value) {
    createOpen.value = false;
    return;
  }
  settingsOpen.value = false;
  sidebarOpen.value = false;
}
function openSettings() {
  settingsTab.value = 'connection';
  endpointDraft.value = endpoint.value;
  formError.value = '';
  settingsOpen.value = true;
}
function saveSettings() {
  if (store.connect(endpointDraft.value)) settingsOpen.value = false;
  else formError.value = 'Check the WebSocket endpoint';
}
function logLevelClass(level: string) {
  const value = level.toUpperCase();
  if (value === 'ERROR') return 'log-level--error';
  if (value === 'WARN') return 'log-level--warn';
  return 'log-level--info';
}
onMounted(() => {
  document.addEventListener('keydown', closeOverlaysOnEscape);
  if (!selectedWorkspace.value && workspaces.value[0]) {
    store.selectWorkspace(workspaces.value[0].path);
  }
  store.connect();
});
onBeforeUnmount(() => {
  document.removeEventListener('keydown', closeOverlaysOnEscape);
  store.disconnect();
});
</script>

<template>
  <div class="shell">
    <header class="shell-topbar">
      <button
        class="icon-button shell-sidebar-toggle"
        title="Toggle navigation"
        @click="toggleSidebar"
      >
        <PanelLeft :size="17" />
      </button>
      <div class="shell-brand">
        <div class="brand-mark"><Command :size="16" :stroke-width="2.4" /></div>
        <div class="brand-copy">
          <strong>margatroid</strong>
          <span>{{ connectionLabel }}</span>
        </div>
      </div>
      <div class="shell-view-switch" role="tablist" aria-label="Center view">
        <button
          type="button"
          :class="{ active: viewMode === 'driver' }"
          :aria-selected="viewMode === 'driver'"
          @click="viewMode = 'driver'"
        >
          <Bot :size="13" /> Driver
        </button>
        <button
          type="button"
          :class="{ active: viewMode === 'info' }"
          :aria-selected="viewMode === 'info'"
          @click="viewMode = 'info'"
        >
          <Boxes :size="13" /> Info
        </button>
      </div>
      <div class="shell-heading">
        <strong>{{ channelLabel }}</strong>
        <span v-if="selectedAgentState">{{ selectedAgentState.status }}</span>
        <span v-else-if="selectedWorkspace">{{ selectedWorkspace.name }}</span>
      </div>
      <div class="shell-actions">
        <button class="icon-button" title="Connection settings" @click="openSettings">
          <Settings2 :size="17" />
        </button>
        <button
          class="icon-button"
          :class="{ 'icon-button--active': activityOpen }"
          title="Toggle activity"
          @click="activityOpen = !activityOpen"
        >
          <PanelRight :size="18" />
        </button>
      </div>
    </header>

    <div class="shell-body">
      <div v-if="sidebarOpen" class="mobile-scrim" @click="sidebarOpen = false"></div>
      <aside
        v-if="!sidebarCollapsed"
        :class="['workspace-sidebar', 'shell-sidebar', { 'workspace-sidebar--open': sidebarOpen }]"
      >
        <div class="sidebar-heading shell-sidebar-heading">
          <span>Workspaces</span>
          <button class="icon-button" title="Create workspace" @click="openCreateWorkspace">
            <Plus :size="15" />
          </button>
        </div>
        <nav class="sidebar-scroll" aria-label="Workspaces">
          <SidebarTree
            :model="sidebarTree"
            :pending="workspaceOperationPending"
            :offline="connectionStatus !== 'online'"
            @select-workspace="selectWorkspace"
            @toggle-workspace="store.toggleWorkspace"
            @select-agent="selectMember"
            @add-agent="openAgentDialog"
            @rename-workspace="renameWorkspaceAlias"
            @delete-workspace="deleteWorkspace"
            @start-agent="startSidebarAgent"
            @stop-agent="stopSidebarAgent"
            @rename-agent="renameSidebarAgent"
            @archive-agent="archiveSidebarAgent"
          />
        </nav>
        <div v-if="workspaceNotice" class="sidebar-footnote" role="status">
          {{ workspaceNotice }}
        </div>
      </aside>

      <main class="shell-center">
        <template v-if="viewMode === 'info'">
          <InfoPanel />
        </template>
        <template v-else>
          <div v-if="connectionError" class="inline-error shell-error">
            <span>{{ connectionError }}</span>
            <button class="text-button" @click="openSettings">Configure</button>
          </div>
          <div v-if="driverDocument" class="shell-driver-frame-wrap">
            <iframe
              :key="iframeKey"
              class="shell-driver-frame"
              :srcdoc="driverDocument"
              sandbox="allow-scripts allow-same-origin allow-forms allow-modals allow-popups"
              title="Driver interface"
            ></iframe>
          </div>
          <div v-else class="shell-empty">
            <div class="empty-icon"><MessageSquare :size="22" /></div>
            <strong>{{ channelLabel }}</strong>
            <span>{{ driverDocumentError || uiState }}</span>
            <span v-if="!selectedAgent && !workspaces.length" class="shell-empty-hint">
              <UsersRound :size="14" /> Start a workspace from the sidebar.
            </span>
          </div>
        </template>
      </main>

      <aside v-if="activityOpen" class="activity-panel shell-activity">
        <div class="activity-header">
          <div>
            <span class="eyebrow">Runtime</span>
            <h2>Activity</h2>
          </div>
          <button class="icon-button" title="Close panel" @click="activityOpen = false">
            <X :size="17" />
          </button>
        </div>
        <div class="activity-tabs" role="tablist" aria-label="Audit panel">
          <button
            type="button"
            :aria-selected="activityTab === 'activity'"
            :class="{ active: activityTab === 'activity' }"
            @click="activityTab = 'activity'"
          >
            Activity
          </button>
          <button
            type="button"
            :aria-selected="activityTab === 'mcl'"
            :class="{ active: activityTab === 'mcl' }"
            @click="activityTab = 'mcl'"
          >
            MCL <span>{{ selectedMclEntries.length }}</span>
          </button>
        </div>
        <div v-if="activityTab === 'mcl'" class="activity-list">
          <div v-if="!selectedMclEntries.length" class="activity-empty">
            No MCL records for the selected agent
          </div>
          <article v-for="record in selectedMclEntries" :key="record.key" class="mcl-entry">
            <code>{{ record.content || 'MCL command' }}</code>
            <small>{{ formatRecordTime(record.timestamp, true) }}</small>
          </article>
        </div>
        <div v-else class="activity-list">
          <div class="activity-filter" role="group" aria-label="Filter activity">
            <button
              type="button"
              :class="{ active: logFilter === 'all' }"
              @click="logFilter = 'all'"
            >
              All <span>{{ activityLogs.length }}</span>
            </button>
            <button
              type="button"
              :class="{ active: logFilter === 'errors' }"
              @click="logFilter = 'errors'"
            >
              Errors
              <span>{{
                activityLogs.filter((log) => log.level === 'ERROR' || log.level === 'WARN').length
              }}</span>
            </button>
            <button
              v-if="!agentLogHistoryLoaded"
              type="button"
              class="activity-history"
              @click="store.loadLogHistory()"
            >
              Load history
            </button>
          </div>
          <div v-if="!displayedLogs.length" class="activity-empty">No activity yet</div>
          <article v-for="log in displayedLogs" :key="log.key" class="log-entry">
            <span :class="['log-level', logLevelClass(log.level)]">{{ log.level }}</span>
            <div class="log-copy">
              <p>{{ log.message }}</p>
              <small>{{ log.target }} · {{ formatRecordTime(log.timestamp, true) }}</small>
              <code v-for="field in log.fields" :key="field.name"
                >{{ field.name }}={{ field.value }}</code
              >
            </div>
          </article>
        </div>
      </aside>
    </div>

    <div v-if="createOpen" class="dialog-backdrop" @click.self="createOpen = false">
      <section class="dialog workspace-create-dialog" role="dialog" aria-modal="true">
        <div class="dialog-header">
          <div>
            <span class="eyebrow">Workspace</span>
            <h2>New Workspace</h2>
          </div>
          <button class="icon-button" title="Close" @click="createOpen = false">
            <X :size="17" />
          </button>
        </div>
        <label class="field-label" for="create-workspace-root">Workspace directory</label>
        <input
          id="create-workspace-root"
          v-model="createRoot"
          class="field-input"
          placeholder="/absolute/project/directory"
          :disabled="createBusy"
          @input="refreshCreateDefaults"
        />
        <div class="workspace-create-auto">
          <div>
            <span>Node name</span><code>{{ createName || 'auto from directory' }}</code>
          </div>
          <div>
            <span>Alias</span><code>{{ createAlias || 'same as node name' }}</code>
          </div>
        </div>
        <button
          class="workspace-create-advanced-toggle"
          type="button"
          @click="createAdvanced = !createAdvanced"
        >
          {{ createAdvanced ? 'Hide advanced options' : 'Advanced options' }}
        </button>
        <div v-if="createAdvanced" class="workspace-create-advanced">
          <label class="field-label" for="create-workspace-name">Node name</label>
          <input
            id="create-workspace-name"
            v-model="createName"
            class="field-input"
            :disabled="createBusy"
            @input="
              createNameTouched = true;
              if (!createAliasTouched) createAlias = createName;
            "
          />
          <label class="field-label" for="create-workspace-alias">Alias</label>
          <input
            id="create-workspace-alias"
            v-model="createAlias"
            class="field-input"
            :disabled="createBusy"
            @input="createAliasTouched = true"
          />
        </div>
        <p v-if="createError" class="form-error">{{ createError }}</p>
        <div class="dialog-actions">
          <button class="secondary-button" :disabled="createBusy" @click="createOpen = false">
            Cancel</button
          ><button class="primary-button" :disabled="createBusy" @click="createManagedWorkspace">
            {{ createBusy ? 'Creating…' : 'Create' }}
          </button>
        </div>
      </section>
    </div>
    <div v-if="addAgentOpen" class="dialog-backdrop" @click.self="addAgentOpen = false">
      <section class="dialog add-agent-dialog" role="dialog" aria-modal="true">
        <div class="dialog-header">
          <div>
            <span class="eyebrow">Agent</span>
            <h2>Add Agent</h2>
            <span class="workspace-edit-address">{{ addAgentWorkspace }}</span>
          </div>
          <button class="icon-button" title="Close" @click="addAgentOpen = false">
            <X :size="17" />
          </button>
        </div>
        <label class="field-label">Driver</label>
        <FormSelect
          v-model="newAgentTool"
          :options="driverOptions"
          :disabled="addAgentBusy"
          @change="applyAgentDefaults"
        />
        <div class="workspace-create-auto">
          <div>
            <span>Node name</span
            ><code>{{
              addAgentTemporary ? 'temporary (no node name)' : newAgentNode || 'auto from Driver'
            }}</code>
          </div>
          <div>
            <span>Alias</span><code>{{ newAgentAlias || 'same as node name' }}</code>
          </div>
        </div>
        <button
          class="workspace-create-advanced-toggle"
          type="button"
          @click="addAgentAdvanced = !addAgentAdvanced"
        >
          {{ addAgentAdvanced ? 'Hide advanced options' : 'Advanced options' }}
        </button>
        <div v-if="addAgentAdvanced" class="workspace-create-advanced">
          <label class="field-label add-agent-node-label">
            <span>Node name</span>
            <span class="add-agent-temporary"
              ><input v-model="addAgentTemporary" type="checkbox" @change="applyAgentDefaults" />
              Temporary agent</span
            >
          </label>
          <input
            v-model="newAgentNode"
            class="field-input"
            :disabled="addAgentBusy || addAgentTemporary"
            @input="
              addAgentNodeTouched = true;
              if (!addAgentAliasTouched) newAgentAlias = newAgentNode;
            "
          />
          <label class="field-label">Alias</label>
          <input
            v-model="newAgentAlias"
            class="field-input"
            :disabled="addAgentBusy"
            @input="addAgentAliasTouched = true"
          />
          <label class="field-label">Sub working directory</label>
          <input
            v-model="newAgentPath"
            class="field-input"
            placeholder="."
            :disabled="addAgentBusy"
          />
        </div>
        <p v-if="addAgentError" class="form-error">{{ addAgentError }}</p>
        <div class="dialog-actions">
          <button class="secondary-button" :disabled="addAgentBusy" @click="addAgentOpen = false">
            Cancel
          </button>
          <button
            class="primary-button"
            :disabled="addAgentBusy || !newAgentTool"
            @click="startNewAgentFromDialog"
          >
            {{ addAgentBusy ? 'Starting…' : 'Start Agent' }}
          </button>
        </div>
      </section>
    </div>
    <div v-if="settingsOpen" class="dialog-backdrop" @click.self="settingsOpen = false">
      <section
        class="dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="shell-settings-title"
      >
        <div class="dialog-header">
          <div>
            <span class="eyebrow">Daemon</span>
            <h2 id="shell-settings-title">Settings</h2>
          </div>
          <button class="icon-button" title="Close" @click="settingsOpen = false">
            <X :size="17" />
          </button>
        </div>
        <div class="settings-tabs" role="tablist" aria-label="Settings sections">
          <button
            type="button"
            :class="{ active: settingsTab === 'connection' }"
            :aria-selected="settingsTab === 'connection'"
            @click="settingsTab = 'connection'"
          >
            Connection
          </button>
          <button
            type="button"
            :class="{ active: settingsTab === 'models' }"
            :aria-selected="settingsTab === 'models'"
            @click="settingsTab = 'models'"
          >
            Models
          </button>
        </div>
        <template v-if="settingsTab === 'connection'">
          <label class="field-label" for="shell-endpoint">WebSocket endpoint</label>
          <input
            id="shell-endpoint"
            v-model="endpointDraft"
            class="field-input"
            placeholder="ws://127.0.0.1:3939/ws"
            @keydown.enter="saveSettings"
          />
          <p v-if="formError" class="form-error">{{ formError }}</p>
          <div class="dialog-actions">
            <button class="secondary-button" @click="settingsOpen = false">Cancel</button>
            <button class="primary-button" @click="saveSettings">Connect</button>
          </div>
        </template>
        <ModelConfigPanel v-else />
      </section>
    </div>
  </div>
</template>
