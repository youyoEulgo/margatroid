<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { storeToRefs } from 'pinia';
import {
  Activity,
  Bot,
  Check,
  ChevronDown,
  CircleAlert,
  ChevronRight,
  CloudOff,
  Command,
  FolderKanban,
  Eye,
  EyeOff,
  LockKeyhole,
  Plus,
  Menu,
  MessageSquare,
  PanelRight,
  Send,
  Square,
  Settings2,
  UserRound,
  UsersRound,
  Wifi,
  Wrench,
  X,
  Zap,
} from 'lucide-vue-next';

import {
  useWorkbenchStore,
  resourceName,
  workspaceKey,
  type ConversationEntry,
  type RuntimeLog,
} from '@/stores/workbench';
import { renderMarkdown } from '@/lib/markdown';

const store = useWorkbenchStore();
const {
  endpoint,
  connectionStatus,
  connectionError,
  workspaces,
  selectedWorkspace,
  selectedAgent,
  selectedAgentName,
  selectedAgentState,
  selectedAgentReady,
  selectedAgentWorking,
  resourceVisibility,
  visibleMessages,
  logs,
  lastFailures,
} = storeToRefs(store);

const draft = ref('');
const pendingSkill = ref<string | null>(null);
const submittingSkill = ref(false);
const draftInput = ref<HTMLTextAreaElement | null>(null);
const messageList = ref<HTMLElement | null>(null);
const conversationPane = ref<HTMLElement | null>(null);
const composerFooter = ref<HTMLElement | null>(null);
let composerObserver: ResizeObserver | undefined;
const toolManager = ref<HTMLElement | null>(null);
const sidebarOpen = ref(false);
const activityOpen = ref(false);
const toolsOpen = ref(false);
const composerActionsOpen = ref(false);
const settingsOpen = ref(false);
const logFilter = ref<'all' | 'errors'>('all');
const endpointDraft = ref(endpoint.value);
const formError = ref('');

const displayedLogs = computed(() => {
  if (logFilter.value === 'errors') {
    return logs.value.filter((entry) => entry.level === 'ERROR' || entry.level === 'WARN');
  }
  return logs.value;
});

const memberList = computed(() => {
  const workspace = selectedWorkspace.value;
  if (!workspace) return [];
  return workspace.agents.filter((agent) => agent !== workspace.manager);
});

const selectedAgentFailure = computed(() => {
  const workspace = selectedWorkspace.value;
  const agent = selectedAgent.value || workspace?.manager;
  if (!workspace || !agent) return null;
  return lastFailures.value.get(`${workspaceKey(workspace)}:${agent}`) ?? null;
});

const connectionLabel = computed(() => {
  if (connectionStatus.value === 'online') return 'Connected';
  if (connectionStatus.value === 'connecting') return 'Connecting';
  return 'Offline';
});

const resourceGroups = computed(() => {
  const groups = [
    { type: 'skill', label: 'Skills', resources: [] as typeof resourceVisibility.value },
    { type: 'hook', label: 'Hooks', resources: [] as typeof resourceVisibility.value },
    { type: 'tool', label: 'Tools', resources: [] as typeof resourceVisibility.value },
    { type: 'shell', label: 'Shells', resources: [] as typeof resourceVisibility.value },
  ];
  for (const resource of resourceVisibility.value) {
    groups
      .find((group) => resource.resource.startsWith(`${group.type}:`))
      ?.resources.push(resource);
  }
  return groups;
});

const activeToolResources = computed(() => resourceVisibility.value
    .filter(
      (entry) =>
        entry.visible &&
        (entry.resource.startsWith('skill:') || entry.resource.startsWith('hook:')),
    )
    .map((entry) => entry.resource));

function compactTokenNumber(value: number) {
  const units = [
    { threshold: 1_000_000_000_000, suffix: 't' },
    { threshold: 1_000_000_000, suffix: 'b' },
    { threshold: 1_000_000, suffix: 'm' },
    { threshold: 1_000, suffix: 'k' },
  ];
  const unit = units.find(({ threshold }) => value >= threshold);
  if (!unit) return String(value);
  const scaled = value / unit.threshold;
  const formatted = scaled >= 100 ? scaled.toFixed(0) : scaled.toFixed(1).replace(/\.0$/, '');
  return `${formatted}${unit.suffix}`;
}

const tokenUsageItems = computed(() => {
  const state = selectedAgentState.value;
  const input = state?.total_input_tokens ?? 0;
  const output = state?.total_output_tokens ?? 0;
  const cached = state?.total_cache_hit_tokens ?? 0;
  const hitRate = state?.cache_hit_rate ?? 0;
  return [
    {
      label: 'In',
      value: compactTokenNumber(input),
      title: `Input tokens: ${input.toLocaleString()}`,
    },
    {
      label: 'Out',
      value: compactTokenNumber(output),
      title: `Output tokens: ${output.toLocaleString()}`,
    },
    {
      label: 'Cached',
      value: compactTokenNumber(cached),
      title: `Cache-hit tokens: ${cached.toLocaleString()}`,
    },
    {
      label: 'Hit',
      value: `${(hitRate * 100).toFixed(1)}%`,
      title: `Cache hit rate: ${(hitRate * 100).toFixed(2)}%`,
    },
  ];
});

const contextWindowUsage = computed(() => {
  const used = selectedAgentState.value?.last_input_tokens ?? 0;
  const maximum = selectedAgentState.value?.context_window_tokens ?? 0;
  const ratio = maximum > 0 ? Math.min(used / maximum, 1) : 0;
  return {
    ratio,
    dashOffset: 44 * (1 - ratio),
    title:
      maximum > 0
        ? `Context window: ${used.toLocaleString()} / ${maximum.toLocaleString()} tokens (${(ratio * 100).toFixed(1)}%)`
        : 'Context window usage unavailable',
  };
});

const currentChannelLabel = computed(() => {
  const workspace = selectedWorkspace.value;
  if (!workspace) return 'Select a workspace';
  if (selectedAgent.value) return resourceName(selectedAgent.value);
  return workspace.manager ? `${resourceName(workspace.manager)} · manager route` : 'Manager route';
});

const NEAR_BOTTOM_PX = 80;

function shouldFollowMessages() {
  const list = messageList.value;
  if (!list) return false;
  return list.scrollHeight - list.scrollTop - list.clientHeight <= NEAR_BOTTOM_PX;
}

watch(
  () =>
    visibleMessages.value.map(
      (message) => `${message.key}:${message.thinking.length}:${message.content.length}`,
    ),
  () => {
    const follow = shouldFollowMessages();
    nextTick(() => {
      if (follow) scrollMessages();
    });
  },
);

const MAX_DRAFT_HEIGHT = 168;

function autosizeDraft() {
  const input = draftInput.value;
  if (!input) return;
  input.style.height = 'auto';
  input.style.height = `${Math.min(input.scrollHeight, MAX_DRAFT_HEIGHT)}px`;
}

watch(draft, () => nextTick(autosizeDraft));
watch([selectedWorkspace, selectedAgent], () => {
  pendingSkill.value = null;
});

function closeToolsOnOutsideClick(event: MouseEvent) {
  const target = event.target;
  if (
    composerActionsOpen.value &&
    (!(target instanceof Element) || !target.closest('.composer-add-menu'))
  ) {
    composerActionsOpen.value = false;
  }
  if (!toolsOpen.value) return;
  if (target instanceof Node && toolManager.value && !toolManager.value.contains(target)) {
    toolsOpen.value = false;
  }
}

onMounted(() => {
  if (!selectedWorkspace.value && workspaces.value[0]) {
    store.selectWorkspace(workspaceKey(workspaces.value[0]));
  }
  store.connect();
  document.addEventListener('click', closeToolsOnOutsideClick);

  composerObserver = new ResizeObserver(() => {
    const pane = conversationPane.value;
    const footer = composerFooter.value;
    if (!pane || !footer) return;
    pane.style.setProperty('--mg-composer-height', `${footer.offsetHeight}px`);
  });
  if (composerFooter.value) composerObserver.observe(composerFooter.value);
});

onBeforeUnmount(() => {
  document.removeEventListener('click', closeToolsOnOutsideClick);
  composerObserver?.disconnect();
  store.disconnect();
});

async function submitMessage() {
  const text = draft.value.trim();
  if (pendingSkill.value) {
    if (submittingSkill.value) return;
    submittingSkill.value = true;
    try {
      await store.sendSkillInvocation(text, pendingSkill.value);
      draft.value = '';
      pendingSkill.value = null;
      nextTick(scrollMessages);
    } catch (error) {
      connectionError.value = error instanceof Error ? error.message : String(error);
    } finally {
      submittingSkill.value = false;
    }
    return;
  }
  if (!text) return;
  if (!store.sendMessage(text)) return;
  draft.value = '';
  nextTick(scrollMessages);
}

function selectSkill(resource: string) {
  pendingSkill.value = resource;
  composerActionsOpen.value = false;
  nextTick(() => draftInput.value?.focus());
}

function cancelPendingSkill() {
  pendingSkill.value = null;
  nextTick(() => draftInput.value?.focus());
}

function handleDraftBackspace(event: KeyboardEvent) {
  const input = event.currentTarget;
  if (!(input instanceof HTMLTextAreaElement) || !pendingSkill.value) return;
  if (input.selectionStart !== 0 || input.selectionEnd !== 0) return;
  event.preventDefault();
  cancelPendingSkill();
}

function toggleResourceVisibility(resource: string, visible: boolean) {
  store.setResourceVisibility(resource, visible);
}

function selectWorkspace(key: string) {
  store.selectWorkspace(key);
  sidebarOpen.value = false;
  nextTick(scrollMessages);
}

function selectMember(agent: string | null) {
  store.selectAgent(agent);
  sidebarOpen.value = false;
  nextTick(scrollMessages);
}

function openSettings() {
  endpointDraft.value = endpoint.value;
  formError.value = '';
  settingsOpen.value = true;
}

function saveSettings() {
  try {
    if (store.connect(endpointDraft.value)) settingsOpen.value = false;
  } catch (error) {
    formError.value = error instanceof Error ? error.message : String(error);
  }
}

function scrollMessages() {
  messageList.value?.scrollTo({ top: messageList.value.scrollHeight, behavior: 'instant' });
}

/** Let the wheel roll through the composer into the message list. */
function handleComposerWheel(event: WheelEvent) {
  const list = messageList.value;
  if (!list) return;

  const target = event.target;
  if (target instanceof Element) {
    // Keep native scrolling inside the expanded tool manager panel.
    if (target.closest('.tool-manager-panel')) return;
    // Keep native scrolling inside an overflowing textarea draft.
    if (target instanceof HTMLTextAreaElement && target.scrollHeight > target.clientHeight) {
      return;
    }
  }

  event.preventDefault();
  list.scrollTop += event.deltaY;
}

function formatTime(timestamp: number) {
  return new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit' }).format(
    timestamp,
  );
}

function formatLogTime(timestamp: number) {
  return new Intl.DateTimeFormat(undefined, {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  }).format(timestamp);
}

function roleLabel(message: ConversationEntry) {
  if (message.role === 'user') return 'You';
  if (message.role === 'assistant') return resourceName(message.agent);
  if (message.role === 'tool') return 'Tool response';
  if (message.role === 'error') return 'Agent error';
  return 'System';
}

function toolArguments(argumentsText: string) {
  try {
    return JSON.stringify(JSON.parse(argumentsText), null, 2);
  } catch {
    return argumentsText;
  }
}

function logLevelClass(log: RuntimeLog) {
  return `log-level--${log.level.toLowerCase()}`;
}
</script>

<template>
  <div class="app-shell">
    <div v-if="sidebarOpen" class="mobile-scrim" @click="sidebarOpen = false"></div>
    <aside :class="['workspace-sidebar', { 'workspace-sidebar--open': sidebarOpen }]">
      <div class="brand-row">
        <div class="brand-mark">
          <Command :size="17" :stroke-width="2.4" />
        </div>
        <div>
          <strong>margatroid</strong>
          <span>agent workspace</span>
        </div>
        <button
          class="icon-button sidebar-close"
          title="Close navigation"
          @click="sidebarOpen = false"
        >
          <X :size="17" />
        </button>
      </div>

      <button class="connection-strip" title="Open connection settings" @click="openSettings">
        <span :class="['status-dot', `status-dot--${connectionStatus}`]"></span>
        <span class="connection-copy">
          <strong>{{ connectionLabel }}</strong>
          <small>{{ endpoint.replace(/^wss?:\/\//, '') }}</small>
        </span>
        <Settings2 :size="16" />
      </button>

      <div class="sidebar-heading">
        <span>Workspaces</span>
      </div>

      <nav class="workspace-list" aria-label="Workspaces">
        <button
          v-for="workspace in workspaces"
          :key="workspaceKey(workspace)"
          :class="[
            'workspace-item',
            {
              'workspace-item--active':
                selectedWorkspace && workspaceKey(selectedWorkspace) === workspaceKey(workspace),
            },
          ]"
          @click="selectWorkspace(workspaceKey(workspace))"
        >
          <FolderKanban :size="17" />
          <span class="workspace-item__copy">
            <strong>{{ workspace.name }}</strong>
            <small>{{ workspace.project_root }}</small>
          </span>
          <ChevronDown
            v-if="selectedWorkspace && workspaceKey(selectedWorkspace) === workspaceKey(workspace)"
            :size="15"
          />
        </button>
        <div v-if="!workspaces.length" class="sidebar-empty">
          <span>No workspace running</span>
        </div>
      </nav>

      <template v-if="selectedWorkspace">
        <div class="sidebar-heading sidebar-heading--members">
          <span>Members</span>
          <span class="count-badge">{{ selectedWorkspace.agents.length }}</span>
        </div>
        <div class="member-list">
          <button
            :class="['member-item', { 'member-item--active': selectedAgent === null }]"
            @click="selectMember(null)"
          >
            <span class="member-avatar member-avatar--manager">
              <Zap :size="15" />
            </span>
            <span class="member-item__copy">
              <strong>{{ resourceName(selectedWorkspace.manager) || 'Manager' }}</strong>
              <small>default route</small>
            </span>
            <Check v-if="selectedAgent === null" :size="15" />
          </button>
          <button
            v-for="agent in memberList"
            :key="agent"
            :class="['member-item', { 'member-item--active': selectedAgent === agent }]"
            @click="selectMember(agent)"
          >
            <span class="member-avatar">
              <Bot :size="15" />
            </span>
            <span class="member-item__copy">
              <strong>{{ resourceName(agent) }}</strong>
              <small>{{ agent === selectedWorkspace.manager ? 'manager' : 'agent' }}</small>
            </span>
            <Check v-if="selectedAgent === agent" :size="15" />
          </button>
        </div>
      </template>

      <div class="sidebar-footer">
        <button class="footer-action" title="Open connection settings" @click="openSettings">
          <Settings2 :size="16" />
          <span>Connection</span>
        </button>
      </div>
    </aside>

    <main ref="conversationPane" class="conversation-pane">
      <header class="conversation-header">
        <button class="icon-button mobile-menu" title="Open navigation" @click="sidebarOpen = true">
          <Menu :size="19" />
        </button>
        <div class="conversation-heading">
          <div class="channel-avatar">
            <UserRound :size="17" />
          </div>
          <div>
            <div class="channel-title">{{ currentChannelLabel }}</div>
            <div class="channel-subtitle">
              <span v-if="selectedWorkspace">{{ selectedWorkspace.name }}</span>
              <span v-if="selectedWorkspace">·</span>
              <span>{{ selectedAgentState?.status || connectionLabel.toLowerCase() }}</span>
            </div>
          </div>
        </div>
        <div class="header-actions">
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

      <section ref="messageList" class="message-list" aria-live="polite">
        <div v-if="!selectedWorkspace" class="empty-state">
          <div class="empty-icon">
            <UsersRound :size="23" />
          </div>
          <strong>Choose a workspace</strong>
        </div>
        <div v-else-if="!visibleMessages.length" class="empty-state empty-state--conversation">
          <div class="empty-icon">
            <MessageSquare :size="22" />
          </div>
          <strong>{{ selectedAgentName }}</strong>
          <span v-if="selectedAgentState?.status === 'failed'">
            {{ selectedAgentState.error || 'Agent initialization failed.' }}
          </span>
          <span v-else-if="selectedAgentState?.status === 'creating'">Agent is starting.</span>
          <span v-else>Conversation is ready.</span>
          <span v-if="selectedAgentFailure" class="agent-failure">
            Last error: {{ selectedAgentFailure.message }}
          </span>
        </div>

        <article
          v-for="message in visibleMessages"
          :key="message.key"
          :class="['message-row', `message-row--${message.role}`]"
        >
          <div class="message-avatar">
            <UserRound v-if="message.role === 'user'" :size="16" />
            <Bot v-else-if="message.role === 'assistant'" :size="16" />
            <Wrench v-else-if="message.role === 'tool'" :size="15" />
            <CircleAlert v-else-if="message.role === 'error'" :size="15" />
            <Zap v-else :size="15" />
          </div>
          <div class="message-body">
            <div class="message-meta">
              <strong>{{ roleLabel(message) }}</strong>
              <span>{{ formatTime(message.timestamp) }}</span>
            </div>
            <details v-if="message.thinking" class="message-thinking">
              <summary>Thinking</summary>
              <p>{{ message.thinking }}</p>
            </details>
            <details v-if="message.role === 'tool' && message.content" class="tool-response">
              <summary>
                <span class="tool-response-label">Tool response</span>
                <span class="tool-response-summary">{{ message.content }}</span>
              </summary>
              <pre class="tool-response-detail">{{ message.content }}</pre>
            </details>
            <div
              v-else-if="message.content"
              class="message-content"
              v-html="renderMarkdown(message.content)"
            ></div>
            <div v-if="message.toolCalls.length" class="tool-call-list">
              <details v-for="tool in message.toolCalls" :key="tool.id" class="tool-call-row">
                <summary>
                  <Wrench :size="14" />
                  <span>{{ tool.tool_name }}</span>
                  <code v-if="tool.arguments">{{ toolArguments(tool.arguments) }}</code>
                </summary>
                <pre class="tool-call-detail">{{
                  tool.arguments ? toolArguments(tool.arguments) : '{}'
                }}</pre>
              </details>
            </div>
          </div>
        </article>

        <div v-if="connectionError" class="inline-error">
          <CloudOff :size="16" />
          <span>{{ connectionError }}</span>
          <button class="text-button" @click="openSettings">Configure</button>
        </div>
      </section>

      <footer ref="composerFooter" class="composer" @wheel="handleComposerWheel">
        <form class="composer-box" @submit.prevent="submitMessage">
          <div v-if="pendingSkill" class="composer-call-row">
            <div class="composer-call-block">
              <div class="composer-call-copy">
                <span class="composer-call-kind">Skill</span>
                <strong>{{ resourceName(pendingSkill) }}</strong>
              </div>
              <button type="button" title="Cancel Skill call" @click="cancelPendingSkill">
                <X :size="14" />
              </button>
            </div>
          </div>
          <div class="composer-input-row">
            <textarea
              ref="draftInput"
              v-model="draft"
              :disabled="!selectedWorkspace || connectionStatus !== 'online' || !selectedAgentReady"
              rows="1"
              :placeholder="
                !selectedWorkspace
                  ? 'Select a workspace...'
                  : selectedAgentReady
                    ? selectedAgentWorking
                      ? `${selectedAgentName} is working...`
                      : `Message ${selectedAgentName}...`
                    : 'Agent is not ready...'
              "
              @keydown.enter.exact.prevent="submitMessage"
              @keydown.backspace="handleDraftBackspace"
            ></textarea>
          </div>
          <div class="composer-actions">
            <div class="composer-add-menu">
              <button
                class="composer-add-button"
                type="button"
                title="Add to message"
                :aria-expanded="composerActionsOpen"
                :disabled="!selectedWorkspace || connectionStatus !== 'online' || !selectedAgentReady"
                @click="composerActionsOpen = !composerActionsOpen"
              >
                <Plus :size="18" />
              </button>
              <div v-if="composerActionsOpen" class="composer-add-popover">
                <template
                  v-for="group in resourceGroups.filter(
                    (group) => group.type === 'skill' || group.type === 'hook',
                  )"
                  :key="group.type"
                >
                  <div class="composer-add-heading">{{ group.label }}</div>
                  <button
                    v-for="entry in group.resources.filter((item) => item.visible)"
                    :key="entry.resource"
                    type="button"
                    @click="selectSkill(entry.resource)"
                  >
                    <span class="resource-state-dot is-active" />
                    <span>{{ resourceName(entry.resource) }}</span>
                  </button>
                  <span
                    v-if="!group.resources.some((item) => item.visible)"
                    class="composer-add-empty"
                  >No visible {{ group.label }}</span>
                </template>
              </div>
            </div>
            <button
              v-if="selectedAgentWorking"
              class="send-button send-button--stop"
              type="button"
              title="Stop current turn"
              :disabled="!selectedWorkspace || connectionStatus !== 'online' || !selectedAgentReady"
              @click="store.abortTurn()"
            >
              <Square :size="14" :stroke-width="0" fill="currentColor" />
            </button>
            <button
              v-else
              class="send-button"
              type="submit"
              title="Send message"
              :disabled="
                (!draft.trim() && !pendingSkill) ||
                submittingSkill ||
                !selectedWorkspace ||
                connectionStatus !== 'online' ||
                !selectedAgentReady
              "
            >
              <Send :size="18" />
            </button>
          </div>
        </form>
        <div v-if="selectedWorkspace" ref="toolManager" class="tool-manager">
          <button
            class="tool-manager-trigger"
            type="button"
            :aria-expanded="toolsOpen"
            aria-controls="tool-manager-panel"
            title="Manage agent tools"
            @click="toolsOpen = !toolsOpen"
          >
            <ChevronRight :class="['tool-manager-chevron', { 'is-open': toolsOpen }]" :size="13" />
            <span v-for="resource in activeToolResources" :key="resource" class="active-tool">
              <span class="active-tool-dot"></span>
              <span>{{ resourceName(resource) }}</span>
            </span>
            <span v-if="!activeToolResources.length" class="tool-manager-empty">Tools</span>
          </button>

          <div class="token-usage" aria-label="Agent token usage">
            <span
              v-for="item in tokenUsageItems"
              :key="item.label"
              class="token-usage-item"
              :title="item.title"
            >
              <span>{{ item.label }}</span>
              <strong>{{ item.value }}</strong>
            </span>
            <span
              class="context-window-usage"
              role="img"
              :aria-label="contextWindowUsage.title"
              :title="contextWindowUsage.title"
            >
              <svg viewBox="0 0 18 18" aria-hidden="true">
                <circle class="context-window-track" cx="9" cy="9" r="7" />
                <circle
                  class="context-window-value"
                  cx="9"
                  cy="9"
                  r="7"
                  :style="{ strokeDashoffset: contextWindowUsage.dashOffset }"
                />
              </svg>
            </span>
          </div>

          <div v-if="toolsOpen" id="tool-manager-panel" class="tool-manager-panel">
            <section v-for="group in resourceGroups" :key="group.type" class="tool-resource-group">
              <div class="tool-resource-heading">
                <span>{{ group.label }}</span>
                <span>{{ group.resources.length }}</span>
              </div>
              <div v-if="group.resources.length" class="tool-resource-list">
                <div
                  v-for="entry in group.resources"
                  :key="entry.resource"
                  class="tool-resource-row"
                >
                  <span :class="['resource-state-dot', { 'is-active': entry.visible }]" />
                  <code :title="entry.resource">{{ resourceName(entry.resource) }}</code>
                  <button
                    v-if="entry.default"
                    class="tool-row-action"
                    type="button"
                    :title="entry.visible ? 'Remove visibility' : 'Inject visibility'"
                    :aria-label="entry.visible ? 'Remove visibility' : 'Inject visibility'"
                    :disabled="connectionStatus !== 'online' || !selectedAgentReady"
                    @click="toggleResourceVisibility(entry.resource, !entry.visible)"
                  >
                    <Eye v-if="entry.visible" :size="14" />
                    <EyeOff v-else :size="14" />
                  </button>
                  <span v-else class="tool-row-action is-readonly" title="Runtime-managed resource">
                    <LockKeyhole :size="14" />
                  </span>
                </div>
              </div>
              <span v-else class="tool-resource-empty">None</span>
            </section>
          </div>
        </div>
      </footer>
    </main>

    <aside v-if="activityOpen" class="activity-panel">
      <div class="activity-header">
        <div>
          <span class="eyebrow">Runtime</span>
          <h2>Activity</h2>
        </div>
        <button class="icon-button" title="Close activity" @click="activityOpen = false">
          <X :size="17" />
        </button>
      </div>
      <div class="activity-tabs" role="tablist">
        <button :class="{ active: logFilter === 'all' }" @click="logFilter = 'all'">
          All <span>{{ logs.length }}</span>
        </button>
        <button :class="{ active: logFilter === 'errors' }" @click="logFilter = 'errors'">
          Errors
          <span>{{
            logs.filter((log) => log.level === 'ERROR' || log.level === 'WARN').length
          }}</span>
        </button>
      </div>
      <div class="activity-list">
        <div v-if="!displayedLogs.length" class="activity-empty">
          <Activity :size="18" />
          <span>No runtime events</span>
        </div>
        <div v-for="log in displayedLogs" :key="log.key" class="log-entry">
          <span :class="['log-level', logLevelClass(log)]">{{ log.level }}</span>
          <div class="log-copy">
            <p>{{ log.message }}</p>
            <small>{{ log.target }} · {{ formatLogTime(log.timestamp) }}</small>
            <code v-for="field in log.fields" :key="field.name"
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
          <div>
            <span class="eyebrow">Daemon</span>
            <h2 id="settings-title">Connection</h2>
          </div>
          <button class="icon-button" title="Close" @click="settingsOpen = false">
            <X :size="17" />
          </button>
        </div>
        <label class="field-label" for="endpoint">WebSocket endpoint</label>
        <input
          id="endpoint"
          v-model="endpointDraft"
          class="field-input"
          placeholder="ws://127.0.0.1:3939/ws"
          @keydown.enter="saveSettings"
        />
        <p v-if="formError" class="form-error">{{ formError }}</p>
        <div class="dialog-actions">
          <button class="secondary-button" @click="settingsOpen = false">Cancel</button>
          <button class="primary-button" @click="saveSettings"><Wifi :size="16" /> Connect</button>
        </div>
      </section>
    </div>
  </div>
</template>
