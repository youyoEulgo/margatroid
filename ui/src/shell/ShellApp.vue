<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { storeToRefs } from 'pinia';
import {
  Command,
  Menu,
  MessageSquare,
  PanelRight,
  Settings2,
  UsersRound,
  X,
} from 'lucide-vue-next';
import SidebarTree from '@/components/SidebarTree.vue';
import { useWorkbenchStore } from '@/stores/workbench';
import { buildDriverDocument } from '@/lib/driverUi';
import { formatRecordTime } from '@/lib/time';

const store = useWorkbenchStore();
const {
  endpoint,
  driverContextPath,
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
  agentUiBundle,
  agentUiError,
  agentUiLoading,
  logs,
  selectedSessionLogs,
  selectedMclEntries,
} = storeToRefs(store);

const sidebarOpen = ref(false);
const activityOpen = ref(typeof window === 'undefined' ? true : window.innerWidth > 1180);
const activityTab = ref<'activity' | 'mcl'>('activity');
const logFilter = ref<'all' | 'errors'>('all');
const settingsOpen = ref(false);
const endpointDraft = ref(endpoint.value);
const contextPathDraft = ref(driverContextPath.value);
const formError = ref('');
const driverDocument = ref('');
const driverDocumentError = ref('');
const iframeKey = ref('');

const connectionLabel = computed(() => {
  if (connectionStatus.value === 'online') return 'Connected';
  if (connectionStatus.value === 'connecting') return 'Connecting';
  return 'Offline';
});
const activityLogs = computed(() => [...logs.value, ...selectedSessionLogs.value]);
const displayedLogs = computed(() =>
  logFilter.value === 'errors'
    ? activityLogs.value.filter((entry) => entry.level === 'ERROR' || entry.level === 'WARN')
    : activityLogs.value,
);
const channelLabel = computed(() => {
  if (selectedAgent.value) return selectedAgent.value.split('/').pop()?.split(':')[0] ?? selectedAgent.value;
  if (selectedWorkspace.value) return selectedWorkspace.value.name;
  return 'No agent selected';
});
const uiState = computed(() => {
  if (!selectedAgent.value) return 'Choose an agent to load its Driver UI.';
  if (agentUiError.value) return agentUiError.value;
  if (agentUiLoading.value) return 'Loading Driver UI…';
  if (!agentUiBundle.value) return 'This Driver does not provide a UI.';
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
      contextPath: driverContextPath.value,
    });
    iframeKey.value = `${selectedAgent.value}:${agentUiBundle.value.revision}`;
  } catch (error) {
    driverDocumentError.value = error instanceof Error ? error.message : String(error);
    driverDocument.value = '';
  }
}
watch([agentUiBundle, selectedAgent, endpoint, driverContextPath], renderDriver, { immediate: true });

watch(
  [activityOpen, activityTab, selectedAgent],
  () => {
    if (activityOpen.value && selectedAgentReady.value)
      store.queryRecords(activityTab.value === 'mcl' ? 'mcl' : 'log');
  },
);

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
function openSettings() {
  endpointDraft.value = endpoint.value;
  contextPathDraft.value = driverContextPath.value;
  formError.value = '';
  settingsOpen.value = true;
}
function saveSettings() {
  store.driverContextPath = contextPathDraft.value;
  if (store.connect(endpointDraft.value)) settingsOpen.value = false;
  else formError.value = 'Check the WebSocket endpoint';
}
function logLevelClass(level: string) {
  const value = level.toUpperCase();
  return value === 'ERROR' || value === 'WARN' ? 'is-error' : '';
}
function closeOverlaysOnEscape(event: KeyboardEvent) {
  if (event.key !== 'Escape') return;
  settingsOpen.value = false;
  sidebarOpen.value = false;
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
      <button class="icon-button shell-menu" title="Open navigation" @click="sidebarOpen = true">
        <Menu :size="18" />
      </button>
      <div class="shell-brand">
        <div class="brand-mark"><Command :size="16" :stroke-width="2.4" /></div>
        <div class="brand-copy">
          <strong>margatroid</strong>
          <span>{{ connectionLabel }}</span>
        </div>
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
      <aside :class="['workspace-sidebar', 'shell-sidebar', { 'workspace-sidebar--open': sidebarOpen }]">
        <div class="sidebar-heading shell-sidebar-heading">
          <span>Workspaces</span>
          <button class="icon-button sidebar-close" title="Close navigation" @click="sidebarOpen = false">
            <X :size="17" />
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
            @start-workspace="startWorkspace"
            @restart-workspace="restartWorkspace"
            @stop-workspace="stopWorkspace"
          />
        </nav>
        <div v-if="workspaceNotice" class="sidebar-footnote" role="status">{{ workspaceNotice }}</div>
      </aside>

      <main class="shell-center">
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
          <div v-if="agentUiError" class="shell-driver-banner" role="status">
            {{ agentUiError }} — showing last loaded interface.
          </div>
        </div>
        <div v-else class="shell-empty">
          <div class="empty-icon"><MessageSquare :size="22" /></div>
          <strong>{{ channelLabel }}</strong>
          <span>{{ driverDocumentError || uiState }}</span>
          <span v-if="!selectedAgent && !workspaces.length" class="shell-empty-hint">
            <UsersRound :size="14" /> Start a workspace from the sidebar.
          </span>
          <button v-if="agentUiError && selectedAgent" class="secondary-button" @click="store.refreshAgentUi()">
            Retry
          </button>
        </div>
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
          <div v-if="!selectedMclEntries.length" class="activity-empty">No MCL records for the selected agent</div>
          <article v-for="record in selectedMclEntries" :key="record.key" class="log-row">
            <span class="log-time">{{ formatRecordTime(record.timestamp, true) }}</span>
            <span class="log-level">MCL</span>
            <span class="log-message">{{ record.content || 'MCL command' }}</span>
          </article>
        </div>
        <div v-else class="activity-list">
          <div class="activity-filter" role="group" aria-label="Filter activity">
            <button type="button" :class="{ active: logFilter === 'all' }" @click="logFilter = 'all'">
              All <span>{{ activityLogs.length }}</span>
            </button>
            <button type="button" :class="{ active: logFilter === 'errors' }" @click="logFilter = 'errors'">
              Errors
              <span>{{ activityLogs.filter((log) => log.level === 'ERROR' || log.level === 'WARN').length }}</span>
            </button>
          </div>
          <div v-if="!displayedLogs.length" class="activity-empty">No activity yet</div>
          <article v-for="log in displayedLogs" :key="log.key" class="log-row">
            <span class="log-time">{{ formatRecordTime(log.timestamp, true) }}</span>
            <span :class="['log-level', logLevelClass(log.level)]">{{ log.level }}</span>
            <span class="log-message">{{ log.message }}</span>
          </article>
        </div>
      </aside>
    </div>

    <div v-if="settingsOpen" class="dialog-backdrop" @click.self="settingsOpen = false">
      <section class="dialog" role="dialog" aria-modal="true" aria-labelledby="shell-settings-title">
        <div class="dialog-header">
          <div>
            <span class="eyebrow">Daemon</span>
            <h2 id="shell-settings-title">Connection</h2>
          </div>
          <button class="icon-button" title="Close" @click="settingsOpen = false">
            <X :size="17" />
          </button>
        </div>
        <label class="field-label" for="shell-endpoint">WebSocket endpoint</label>
        <input
          id="shell-endpoint"
          v-model="endpointDraft"
          class="field-input"
          placeholder="ws://127.0.0.1:3939/ws"
          @keydown.enter="saveSettings"
        />
        <label class="field-label" for="shell-context-path">Driver context path</label>
        <input
          id="shell-context-path"
          v-model="contextPathDraft"
          class="field-input"
          placeholder="entry.pending[-1]"
          @keydown.enter="saveSettings"
        />
        <p class="field-hint">
          The context path is passed to the Driver interface when it is loaded.
        </p>
        <p v-if="formError" class="form-error">{{ formError }}</p>
        <div class="dialog-actions">
          <button class="secondary-button" @click="settingsOpen = false">Cancel</button>
          <button class="primary-button" @click="saveSettings">Connect</button>
        </div>
      </section>
    </div>
  </div>
</template>
