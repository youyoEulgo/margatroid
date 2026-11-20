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
  Trash2,
  UsersRound,
  X,
} from 'lucide-vue-next';
import SidebarTree from '@/components/SidebarTree.vue';
import ModelConfigPanel from '@/components/ModelConfigPanel.vue';
import { useWorkbenchStore } from '@/stores/workbench';
import { buildDriverDocument } from '@/lib/driverUi';
import InfoPanel from './InfoPanel.vue';
import type { AgentDefinition } from '@/types/protocol';
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
const workspaceEditorOpen = ref(false);
const workspaceEditorPath = ref('');
const workspaceEditorAddress = ref('');
const workspaceEditorName = ref('');
const editProjectRoot = ref('');
const editAgents = ref<AgentDefinition[]>([]);
const renameAddress = ref('');
const editTab = ref<'definition' | 'rename' | 'delete'>('definition');
const editNotice = ref('');
const editError = ref('');
const editBusy = ref(false);
const deleteArmed = ref(false);

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
function startWorkspace(path: string) {
  void store.startWorkspace(path);
}
function restartWorkspace(path: string) {
  void store.restartWorkspace(path);
}
function stopWorkspace(address: string) {
  void store.stopWorkspace(address);
}
function openWorkspaceEditor(path: string) {
  const snapshot = workspaceSnapshots.value.find((workspace) => workspace.path === path);
  if (!snapshot) return;
  workspaceEditorPath.value = path;
  workspaceEditorAddress.value = snapshot.address ?? store.workspaceAddress(path) ?? '';
  workspaceEditorName.value =
    store.sidebarTree.workspaces.find((workspace) => workspace.path === path)?.name ?? path;
  editProjectRoot.value = snapshot.definition.project_root;
  editAgents.value = snapshot.definition.agents.map((agent) => ({ ...agent }));
  renameAddress.value = workspaceEditorAddress.value;
  editTab.value = 'definition';
  editNotice.value = '';
  editError.value = '';
  editBusy.value = false;
  deleteArmed.value = false;
  workspaceEditorOpen.value = true;
}
function closeWorkspaceEditor() {
  if (editBusy.value) return;
  workspaceEditorOpen.value = false;
}
function addEditAgent() {
  editAgents.value.push({
    name: '',
    driver: '',
    home_root: editProjectRoot.value,
  });
}
function removeEditAgent(index: number) {
  editAgents.value.splice(index, 1);
}
async function saveWorkspaceDefinition() {
  if (!workspaceEditorAddress.value) {
    editError.value = 'This workspace has no address';
    return;
  }
  if (!editProjectRoot.value.trim()) {
    editError.value = 'Project root is required';
    return;
  }
  if (!editAgents.value.length) {
    editError.value = 'At least one member is required';
    return;
  }
  if (
    editAgents.value.some(
      (agent) => !agent.name.trim() || !agent.driver.trim() || !agent.home_root.trim(),
    )
  ) {
    editError.value = 'Each member needs name, driver and home root';
    return;
  }
  editBusy.value = true;
  editError.value = '';
  editNotice.value = '';
  try {
    await store.updateWorkspace(workspaceEditorAddress.value, {
      project_root: editProjectRoot.value.trim(),
      agents: editAgents.value.map((agent) => ({
        name: agent.name.trim(),
        driver: agent.driver.trim(),
        home_root: agent.home_root.trim(),
      })),
    });
    editNotice.value = 'Workspace definition saved.';
  } catch {
    editError.value = 'Workspace update failed';
  } finally {
    editBusy.value = false;
  }
}
async function renameWorkspace() {
  if (!workspaceEditorAddress.value) {
    editError.value = 'This workspace has no address';
    return;
  }
  const next = renameAddress.value.trim();
  if (!/^workspace:[^/\s:]+\/[^/\s:]+:[^/\s:]+$/u.test(next)) {
    editError.value = 'Address must be workspace:scope/name:tag';
    return;
  }
  if (next === workspaceEditorAddress.value) {
    editError.value = 'New address is unchanged';
    return;
  }
  editBusy.value = true;
  editError.value = '';
  editNotice.value = '';
  try {
    const old = workspaceEditorAddress.value;
    await store.renameWorkspace(old, next);
    workspaceEditorAddress.value = next;
    renameAddress.value = next;
    workspaceEditorPath.value = workspaceEditorPath.value.replace(old, next);
    editNotice.value = 'Workspace renamed.';
  } catch {
    editError.value = 'Workspace rename failed';
  } finally {
    editBusy.value = false;
  }
}
async function deleteWorkspace() {
  if (!workspaceEditorAddress.value) {
    editError.value = 'This workspace has no address';
    return;
  }
  if (!deleteArmed.value) {
    deleteArmed.value = true;
    return;
  }
  editBusy.value = true;
  editError.value = '';
  try {
    await store.deleteWorkspace(workspaceEditorAddress.value);
    workspaceEditorOpen.value = false;
  } catch {
    editError.value = 'Workspace delete failed';
  } finally {
    editBusy.value = false;
  }
}
function closeOverlaysOnEscape(event: KeyboardEvent) {
  if (event.key !== 'Escape') return;
  if (workspaceEditorOpen.value) {
    closeWorkspaceEditor();
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
        </div>
        <nav class="sidebar-scroll" aria-label="Workspaces">
          <SidebarTree
            :model="sidebarTree"
            :pending="workspaceOperationPending"
            :offline="connectionStatus !== 'online'"
            @select-workspace="selectWorkspace"
            @toggle-workspace="store.toggleWorkspace"
            @select-agent="selectMember"
            @start-workspace="startWorkspace"
            @restart-workspace="restartWorkspace"
            @stop-workspace="stopWorkspace"
            @edit-workspace="openWorkspaceEditor"
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

    <div v-if="workspaceEditorOpen" class="dialog-backdrop" @click.self="closeWorkspaceEditor">
      <section
        class="dialog workspace-editor-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="workspace-editor-title"
      >
        <div class="dialog-header">
          <div>
            <span class="eyebrow">Workspace</span>
            <h2 id="workspace-editor-title">{{ workspaceEditorName }}</h2>
            <span class="workspace-edit-address">{{ workspaceEditorAddress }}</span>
          </div>
          <button class="icon-button" title="Close" @click="closeWorkspaceEditor">
            <X :size="17" />
          </button>
        </div>

        <div class="settings-tabs" role="tablist" aria-label="Workspace actions">
          <button
            type="button"
            :class="{ active: editTab === 'definition' }"
            :aria-selected="editTab === 'definition'"
            @click="editTab = 'definition'"
          >
            Definition
          </button>
          <button
            type="button"
            :class="{ active: editTab === 'rename' }"
            :aria-selected="editTab === 'rename'"
            @click="editTab = 'rename'"
          >
            Rename
          </button>
          <button
            type="button"
            :class="{ active: editTab === 'delete' }"
            :aria-selected="editTab === 'delete'"
            @click="
              editTab = 'delete';
              deleteArmed = false;
            "
          >
            Delete
          </button>
        </div>

        <template v-if="editTab === 'definition'">
          <label class="field-label" for="workspace-project-root">Project root</label>
          <input
            id="workspace-project-root"
            v-model="editProjectRoot"
            class="field-input"
            placeholder="/absolute/project/root"
            :disabled="editBusy"
          />
          <div class="workspace-agent-editor-heading">
            <span>Members</span>
            <button
              class="secondary-button"
              type="button"
              :disabled="editBusy"
              @click="addEditAgent"
            >
              <Plus :size="14" /> Add member
            </button>
          </div>
          <div class="workspace-agent-editor">
            <div v-for="(agent, index) in editAgents" :key="index" class="workspace-agent-edit-row">
              <label>
                <span>Name</span>
                <input v-model="agent.name" class="field-input" :disabled="editBusy" />
              </label>
              <label>
                <span>Driver</span>
                <input
                  v-model="agent.driver"
                  class="field-input"
                  placeholder="driver:margatroid/coder:dev"
                  :disabled="editBusy"
                />
              </label>
              <label>
                <span>Home root</span>
                <input v-model="agent.home_root" class="field-input" :disabled="editBusy" />
              </label>
              <button
                class="icon-button workspace-agent-remove"
                type="button"
                title="Remove member"
                :disabled="editBusy || editAgents.length <= 1"
                @click="removeEditAgent(index)"
              >
                <Trash2 :size="15" />
              </button>
            </div>
            <p v-if="!editAgents.length" class="field-hint">No members configured.</p>
          </div>
          <p v-if="editError" class="form-error">{{ editError }}</p>
          <p v-if="editNotice" class="field-hint">{{ editNotice }}</p>
          <div class="dialog-actions">
            <button class="secondary-button" :disabled="editBusy" @click="closeWorkspaceEditor">
              Close
            </button>
            <button class="primary-button" :disabled="editBusy" @click="saveWorkspaceDefinition">
              {{ editBusy ? 'Saving…' : 'Save changes' }}
            </button>
          </div>
        </template>

        <template v-else-if="editTab === 'rename'">
          <p class="field-hint">
            Workspace must be fully stopped. Renaming moves its Agent configuration, history and
            snapshots, but not the project directory.
          </p>
          <label class="field-label" for="workspace-rename-address">New address</label>
          <input
            id="workspace-rename-address"
            v-model="renameAddress"
            class="field-input"
            placeholder="workspace:margatroid/renamed:dev"
            :disabled="editBusy"
          />
          <p v-if="editError" class="form-error">{{ editError }}</p>
          <p v-if="editNotice" class="field-hint">{{ editNotice }}</p>
          <div class="dialog-actions">
            <button class="secondary-button" :disabled="editBusy" @click="closeWorkspaceEditor">
              Close
            </button>
            <button class="primary-button" :disabled="editBusy" @click="renameWorkspace">
              {{ editBusy ? 'Renaming…' : 'Rename workspace' }}
            </button>
          </div>
        </template>

        <template v-else>
          <p class="workspace-edit-warning">
            Deleting the Workspace removes its managed definition, Agent configuration, history and
            snapshots. The project root directory is not deleted.
          </p>
          <p v-if="editError" class="form-error">{{ editError }}</p>
          <div class="dialog-actions">
            <button class="secondary-button" :disabled="editBusy" @click="closeWorkspaceEditor">
              Cancel
            </button>
            <button
              class="primary-button workspace-delete-button"
              :disabled="editBusy"
              @click="deleteWorkspace"
            >
              <Trash2 :size="15" />
              {{ deleteArmed ? 'Confirm delete' : 'Delete workspace' }}
            </button>
          </div>
        </template>
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
