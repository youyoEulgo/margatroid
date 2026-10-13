<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { storeToRefs } from 'pinia';
import {
  Bot,
  Check,
  CloudOff,
  Command,
  FolderKanban,
  Menu,
  MessageSquare,
  PanelRight,
  Send,
  Settings2,
  ShieldCheck,
  UserRound,
  Wifi,
  Wrench,
  X,
} from 'lucide-vue-next';
import { useWorkbenchStore, resourceName, workspaceKey, parseArguments } from '@/stores/workbench';
import { renderMarkdown } from '@/lib/markdown';
const store = useWorkbenchStore();
const {
  endpoint,
  driverContextPath,
  connectionStatus,
  connectionError,
  workspaces,
  selectedWorkspace,
  selectedAgent,
  selectedAgentName,
  selectedAgentState,
  selectedAgentReady,
  loadedResources,
  visibleMessages,
  tokenUsage,
  logs,
} = storeToRefs(store);
const sidebarOpen = ref(false);
const activityOpen = ref(false);
const settingsOpen = ref(false);
const resourcesOpen = ref(false);
const composerMode = ref<'message' | 'mcl'>('mcl');
const draft = ref('');
const argumentsDraft = ref('[]');
const submitting = ref(false);
const result = ref('');
const endpointDraft = ref('');
const contextPathDraft = ref('');
const formError = ref('');
const messageList = ref<HTMLElement | null>(null);
const memberList = computed(
  () =>
    selectedWorkspace.value?.agents.filter((agent) => agent !== selectedWorkspace.value?.manager) ??
    [],
);
const connectionLabel = computed(() =>
  connectionStatus.value === 'online'
    ? 'Connected'
    : connectionStatus.value === 'connecting'
      ? 'Connecting'
      : 'Offline',
);
const canSubmit = computed(
  () =>
    selectedAgentReady.value &&
    connectionStatus.value === 'online' &&
    !!draft.value.trim() &&
    !submitting.value &&
    (composerMode.value === 'mcl' || !!driverContextPath.value),
);
function openSettings() {
  endpointDraft.value = endpoint.value;
  contextPathDraft.value = driverContextPath.value;
  formError.value = '';
  settingsOpen.value = true;
}
function saveSettings() {
  driverContextPath.value = contextPathDraft.value.trim();
  if (store.connect(endpointDraft.value)) settingsOpen.value = false;
  else formError.value = connectionError.value;
}
async function submit() {
  if (!canSubmit.value) return;
  submitting.value = true;
  result.value = '';
  try {
    const value =
      composerMode.value === 'mcl'
        ? await store.executeMclCommand(draft.value, parseArguments(argumentsDraft.value))
        : await store.sendMessage(draft.value);
    result.value = JSON.stringify(value, null, 2) ?? 'Command completed';
    if (composerMode.value === 'message') draft.value = '';
  } catch (error) {
    result.value = error instanceof Error ? error.message : String(error);
  } finally {
    submitting.value = false;
  }
}
function formatTime(timestamp: number) {
  return timestamp > 0
    ? new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit' }).format(timestamp)
    : 'Time unavailable';
}
watch(
  () => visibleMessages.value.map((m) => `${m.key}:${m.content.length}:${m.thinking.length}`),
  async () => {
    const list = messageList.value;
    const follow = list && list.scrollHeight - list.scrollTop - list.clientHeight < 80;
    await nextTick();
    if (follow) list.scrollTop = list.scrollHeight;
  },
);
onMounted(() => store.connect());
onBeforeUnmount(() => store.disconnect());
</script>

<template>
  <div class="app-shell">
    <div v-if="sidebarOpen" class="mobile-scrim" @click="sidebarOpen = false"></div>
    <aside :class="['workspace-sidebar', { 'workspace-sidebar--open': sidebarOpen }]">
      <div class="brand-row">
        <div class="brand-mark"><Command :size="17" /></div>
        <div><strong>margatroid</strong><span>MCL workspace</span></div>
        <button class="icon-button sidebar-close" @click="sidebarOpen = false">
          <X :size="17" />
        </button>
      </div>
      <button class="connection-strip" @click="openSettings">
        <span :class="['status-dot', `status-dot--${connectionStatus}`]"></span
        ><span class="connection-copy"
          ><strong>{{ connectionLabel }}</strong
          ><small>{{ endpoint }}</small></span
        ><Settings2 :size="16" />
      </button>
      <div class="sidebar-heading">Workspaces</div>
      <nav class="workspace-list" aria-label="Workspaces">
        <button
          v-for="workspace in workspaces"
          :key="workspace.id"
          :class="[
            'workspace-item',
            { 'workspace-item--active': workspace.id === selectedWorkspace?.id },
          ]"
          @click="
            store.selectWorkspace(workspaceKey(workspace));
            sidebarOpen = false;
          "
        >
          <FolderKanban :size="17" /><span class="workspace-item__copy"
            ><strong>{{ workspace.name }}</strong
            ><small>{{ workspace.project_root }}</small></span
          >
        </button>
        <div v-if="!workspaces.length" class="sidebar-empty">No workspace running</div>
      </nav>
      <template v-if="selectedWorkspace">
        <div class="sidebar-heading sidebar-heading--members">Members</div>
        <div class="member-list">
          <button
            :class="['member-item', { 'member-item--active': selectedAgent === null }]"
            @click="store.selectAgent(null)"
          >
            <Bot :size="15" /><span class="member-item__copy"
              ><strong>{{ resourceName(selectedWorkspace.manager) }}</strong
              ><small>manager route</small></span
            ><Check v-if="selectedAgent === null" :size="15" />
          </button>
          <button
            v-for="agent in memberList"
            :key="agent"
            :class="['member-item', { 'member-item--active': selectedAgent === agent }]"
            @click="store.selectAgent(agent)"
          >
            <Bot :size="15" /><span class="member-item__copy"
              ><strong>{{ resourceName(agent) }}</strong
              ><small>{{ agent }}</small></span
            >
          </button>
        </div>
      </template>
      <div class="sidebar-footer">
        <button class="footer-action" @click="openSettings">
          <Settings2 :size="16" /> Connection & driver
        </button>
      </div>
    </aside>
    <main class="conversation-pane">
      <header class="conversation-header">
        <button class="icon-button mobile-menu" @click="sidebarOpen = true">
          <Menu :size="19" />
        </button>
        <div class="conversation-heading">
          <div class="channel-avatar"><Bot :size="17" /></div>
          <div>
            <div class="channel-title">{{ selectedAgentName }}</div>
            <div class="channel-subtitle">
              {{ selectedWorkspace?.name }} · {{ selectedAgentState?.status || connectionLabel }}
            </div>
          </div>
        </div>
        <div class="header-actions">
          <button class="icon-button" title="Activity" @click="activityOpen = !activityOpen">
            <PanelRight :size="18" />
          </button>
        </div>
      </header>
      <section ref="messageList" class="message-list" aria-live="polite">
        <div v-if="!visibleMessages.length" class="empty-state">
          <MessageSquare :size="23" /><strong>{{
            selectedWorkspace ? 'History is empty' : 'Select a workspace'
          }}</strong
          ><span>Use the MCL workbench to issue explicit commands.</span>
        </div>
        <template v-for="message in visibleMessages" :key="message.key">
          <details v-if="message.role === 'record'" class="record-note">
            <summary>{{ message.content || message.kind }}</summary>
            <pre>{{ JSON.stringify(message.payload, null, 2) }}</pre>
          </details>
          <article
            v-else
            :class="[
              'message-row',
              `message-row--${message.role}`,
              { 'message-row--failed': message.failed },
            ]"
          >
            <div class="message-avatar">
              <UserRound v-if="message.role === 'user'" :size="16" /><Wrench
                v-else-if="message.role === 'tool'"
                :size="16"
              /><Bot v-else :size="16" />
            </div>
            <div class="message-body">
              <div class="message-meta">
                <strong>{{
                  message.role === 'system'
                    ? 'System'
                    : message.role === 'user'
                      ? 'You'
                      : message.role === 'assistant'
                        ? resourceName(message.agent)
                        : message.role
                }}</strong
                ><span>{{ formatTime(message.timestamp) }}</span>
              </div>
              <details v-if="message.thinking" class="message-thinking">
                <summary>Thinking</summary>
                <p>{{ message.thinking }}</p>
              </details>
              <details v-if="message.role === 'tool'" class="tool-response">
                <summary>Tool response {{ message.failed ? '(failed)' : '' }}</summary>
                <pre>{{ message.content }}</pre>
              </details>
              <div v-else class="message-content" v-html="renderMarkdown(message.content)"></div>
              <div v-if="message.toolCalls.length" class="tool-call-list">
                <details v-for="tool in message.toolCalls" :key="tool.id" class="tool-call-row">
                  <summary><Wrench :size="14" /> {{ tool.tool_name }}</summary>
                  <pre>{{ tool.arguments }}</pre>
                </details>
              </div>
            </div>
          </article>
        </template>
        <div v-if="connectionError" class="inline-error">
          <CloudOff :size="16" />{{ connectionError
          }}<button class="text-button" @click="openSettings">Configure</button>
        </div>
      </section>
      <footer class="composer workbench-composer">
        <div class="workbench-mode">
          <button :class="{ active: composerMode === 'mcl' }" @click="composerMode = 'mcl'">
            MCL command</button
          ><button
            :class="{ active: composerMode === 'message' }"
            @click="composerMode = 'message'"
          >
            Inject user message</button
          ><span
            >Queue {{ selectedAgentState?.queued_commands ?? 0 }} · Turn
            {{ selectedAgentState?.turn || 'none' }}</span
          >
        </div>
        <p v-if="composerMode === 'message'" class="workbench-hint">
          {{
            driverContextPath
              ? `INJECT ? TO ${driverContextPath}`
              : 'Configure a driver context path in settings. No context location or wake command is assumed.'
          }}
        </p>
        <form class="composer-box" @submit.prevent="submit">
          <div class="composer-input-row">
            <textarea
              v-model="draft"
              rows="2"
              :disabled="!selectedAgentReady || connectionStatus !== 'online'"
              :placeholder="
                composerMode === 'mcl'
                  ? 'Enter one MCL instruction…'
                  : 'User message for the configured driver context…'
              "
              @keydown.ctrl.enter.prevent="submit"
            ></textarea>
          </div>
          <label v-if="composerMode === 'mcl'" class="workbench-arguments"
            >Arguments (JSON arrays, one per ?)<textarea
              v-model="argumentsDraft"
              rows="2"
              spellcheck="false"
              placeholder="[[1, 2], [3]]"
            ></textarea>
          </label>
          <div class="composer-actions">
            <span class="workbench-hint">Ctrl+Enter · explicit single instruction</span
            ><button
              class="send-button"
              type="submit"
              :disabled="!canSubmit"
              :title="submitting ? 'Awaiting command result' : 'Execute command'"
            >
              <Send :size="18" />
            </button>
          </div>
        </form>
        <details v-if="result" class="workbench-result" open>
          <summary>Command result</summary>
          <pre>{{ result }}</pre>
        </details>
        <div class="tool-manager">
          <button class="tool-manager-trigger" @click="resourcesOpen = !resourcesOpen">
            Loaded resources & sandbox
          </button>
          <div class="token-usage">
            <span>In {{ tokenUsage.input }}</span
            ><span>Out {{ tokenUsage.output }}</span
            ><span>Cached {{ tokenUsage.cached }}</span>
          </div>
          <div v-if="resourcesOpen" class="tool-manager-panel">
            <section class="tool-resource-group">
              <div class="tool-resource-heading"><ShieldCheck :size="14" /> Sandbox</div>
              <code>{{ selectedAgentState?.sandbox || 'None' }}</code>
              <p class="workbench-hint">
                Read-only status. Change sandbox with an explicit MCL command.
              </p>
            </section>
            <section class="tool-resource-group">
              <div class="tool-resource-heading">Loaded resource sets</div>
              <code v-for="resource in loadedResources" :key="resource" class="tool-resource-row">{{
                resource
              }}</code
              ><span v-if="!loadedResources.length" class="tool-resource-empty"
                >Backend does not publish loaded resource sets.</span
              >
            </section>
          </div>
        </div>
      </footer>
    </main>
    <aside v-if="activityOpen" class="activity-panel">
      <div class="activity-header">
        <h2>Activity</h2>
        <button class="icon-button" @click="activityOpen = false"><X :size="17" /></button>
      </div>
      <div class="activity-list">
        <div v-for="log in logs" :key="log.key" class="log-entry">
          <span class="log-level">{{ log.level }}</span>
          <div class="log-copy">
            <p>{{ log.message }}</p>
            <small>{{ log.target }} · {{ formatTime(log.timestamp) }}</small
            ><code v-for="field in log.fields" :key="field.name"
              >{{ field.name }}={{ field.value }}</code
            >
          </div>
        </div>
      </div>
      <button class="activity-clear" @click="store.clearLogs">Clear activity</button>
    </aside>
    <div v-if="settingsOpen" class="dialog-backdrop" @click.self="settingsOpen = false">
      <section class="dialog" role="dialog" aria-modal="true" aria-labelledby="settings-title">
        <div class="dialog-header">
          <h2 id="settings-title">Connection & driver</h2>
          <button class="icon-button" @click="settingsOpen = false"><X :size="17" /></button>
        </div>
        <label class="field-label" for="endpoint">WebSocket endpoint</label
        ><input id="endpoint" v-model="endpointDraft" class="field-input" /><label
          class="field-label"
          for="context-path"
          >Driver context path (optional)</label
        ><input
          id="context-path"
          v-model="contextPathDraft"
          class="field-input"
          placeholder="Path defined by your driver"
        />
        <p class="workbench-hint">
          Only used for user-message INJECT. It is a driver convention, not a protocol default.
          Injection alone does not start inference.
        </p>
        <p v-if="formError" class="form-error">{{ formError }}</p>
        <div class="dialog-actions">
          <button class="secondary-button" @click="settingsOpen = false">Cancel</button
          ><button class="primary-button" @click="saveSettings">
            <Wifi :size="16" /> Save & connect
          </button>
        </div>
      </section>
    </div>
  </div>
</template>

<style scoped>
.conversation-pane .workbench-composer {
  position: relative;
  right: auto;
  bottom: auto;
  left: auto;
  flex-shrink: 0;
  max-height: 50vh;
  overflow: auto;
}
.workbench-mode {
  display: flex;
  gap: 12px;
  align-items: center;
  padding: 8px 0;
  font-size: 12px;
  flex-wrap: wrap;
}
.workbench-mode button {
  border: 1px solid var(--mg-border, #ddd);
  padding: 5px 8px;
  border-radius: 6px;
  background: transparent;
  color: inherit;
}
.workbench-mode button.active {
  font-weight: 700;
  background: #eceff3;
  color: #222;
}
.workbench-hint {
  font-size: 12px;
  opacity: 0.65;
  margin: 5px 0;
}
.workbench-arguments {
  display: block;
  margin: 0 14px;
  font-size: 12px;
}
.workbench-arguments textarea {
  display: block;
  width: 100%;
  border: 0;
  background: transparent;
  color: inherit;
  font-family: monospace;
  resize: vertical;
  padding: 8px 0;
}
.workbench-result {
  font-size: 12px;
  max-height: 150px;
  overflow: auto;
  margin: 8px 0;
}
.workbench-result pre,
.record-note pre {
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}
.token-usage {
  gap: 8px;
}
</style>
