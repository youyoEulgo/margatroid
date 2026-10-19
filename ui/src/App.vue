<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { storeToRefs } from 'pinia';
import {
  Activity,
  Bot,
  CircleAlert,
  ChevronRight,
  CloudOff,
  Command,
  Eye,
  EyeOff,
  LockKeyhole,
  Menu,
  MessageSquare,
  PanelRight,
  Plus,
  Send,
  Square,
  Circle,
  CircleDot,
  Settings2,
  ShieldCheck,
  ShieldOff,
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
} from '@/stores/workbench';
import SidebarTree from '@/components/SidebarTree.vue';
import CatalogDialog, { type DialogRequest, type DialogValue } from '@/components/CatalogDialog.vue';
import { renderMarkdown } from '@/lib/markdown';
import { formatRecordTime } from '@/lib/time';
import { IMAGE_ACCEPT, importImage, imageSource, validateImageBudget, wireImages, metadataText, type DraftImage } from '@/lib/images';

const store = useWorkbenchStore();
const {
  endpoint,
  driverContextPath,
  tokenUsage,
  contextTokens,
  contextWindowTokens,
  connectionStatus,
  connectionError,
  workspaces,
  selectedCatalogEntry,
  workspaceOperationPending,
  workspaceNotice,
  selectedWorkspace,
  selectedAgent,
  selectedAgentName,
  selectedAgentState,
  selectedAgentReady,
  selectedAgentIdle,
  abortPending,
  selectedAgentWorking,
  sidebarTree,
  drivers,
  resourceVisibility,
  sandboxOptions,
  visibleMessages,
  logs,
  lastFailures,
} = storeToRefs(store);
const canAbortTurn = computed(() => store.canAbortTurn);

const draft = ref('');
const draftImages = ref<DraftImage[]>([]);
const imagePicker = ref<HTMLInputElement | null>(null);
const uploadMenuOpen = ref(false);
const composerError = ref('');
const importingImages = ref(false);
let draftGeneration = 0;
let pickerGeneration = 0;
function openImagePicker() {
  uploadMenuOpen.value = false;
  pickerGeneration = draftGeneration;
  imagePicker.value?.click();
}
const composerRoute = computed(() => `${endpoint.value}:${selectedWorkspace.value?.id}:${selectedAgent.value}:${selectedAgentState.value?.agent_instance}`);
watch(composerRoute, () => {
  draftGeneration++;
  draft.value = '';
  draftImages.value = [];
  composerError.value = '';
  importingImages.value = false;
  submittingMessage.value = false;
  sendStage.value = null;
  uploadMenuOpen.value = false;
  if (imagePicker.value) imagePicker.value.value = '';
}, { flush: 'sync' });
const canEditComposer = computed(() => !!selectedWorkspace.value && connectionStatus.value === 'online' && selectedAgentIdle.value && !submittingMessage.value);
async function importFiles(files: File[]) {
  if (!canEditComposer.value || importingImages.value || !files.length) return;
  const generation = draftGeneration;
  importingImages.value = true;
  composerError.value = '';
  try {
    validateImageBudget([...draftImages.value.map((image) => image.size), ...files.map((file) => file.size)]);
    const imported: DraftImage[] = [];
    for (const file of files) imported.push(await importImage(file));
    if (generation === draftGeneration) draftImages.value.push(...imported);
  } catch (error) {
    if (generation === draftGeneration) composerError.value = error instanceof Error ? error.message : 'Image import failed';
  } finally {
    if (generation === draftGeneration) importingImages.value = false;
  }
}
function pickImages(event: Event) {
  const input = event.target as HTMLInputElement;
  if (pickerGeneration === draftGeneration) void importFiles(Array.from(input.files ?? []));
  input.value = '';
}
function dropImages(event: DragEvent) {
  void importFiles(Array.from(event.dataTransfer?.files ?? []));
}

const submittingMessage = ref(false);
const sendStage = ref<'injecting' | 'waking' | null>(null);
const draftInput = ref<HTMLTextAreaElement | null>(null);
const messageList = ref<HTMLElement | null>(null);
const conversationPane = ref<HTMLElement | null>(null);
const composerFooter = ref<HTMLElement | null>(null);
let composerObserver: ResizeObserver | undefined;
const toolManager = ref<HTMLElement | null>(null);
const sidebarOpen = ref(false);
const activityOpen = ref(false);
const activityTab = ref<'activity' | 'mcl'>('activity');
const toolsOpen = ref(false);
const settingsOpen = ref(false);
const logFilter = ref<'all' | 'errors'>('all');
const endpointDraft = ref(endpoint.value);
const contextPathDraft = ref(driverContextPath.value);
const formError = ref('');
const catalogDialog = ref<DialogRequest | null>(null);
const catalogDialogEl = ref<InstanceType<typeof CatalogDialog> | null>(null);
const catalogDialogBusy = ref(false);

function openCatalogDialog(request: DialogRequest) {
  if (catalogDialogBusy.value || workspaceOperationPending.value) return;
  if (connectionStatus.value !== 'online') {
    connectionError.value = 'Connect to the daemon before changing definitions';
    return;
  }
  catalogDialog.value = request;
  const activeDialog = catalogDialog.value;
  if (request.kind === 'new-session') void store.queryDrivers().catch((error) => {
    if (catalogDialog.value === activeDialog) catalogDialogEl.value?.showError(String(error.message ?? error));
  });
}
const sidebarWorkspace = (key: string) => sidebarTree.value.workspaces.find((workspace) => workspace.key === key);

function requestNewSession(key: string) {
  const workspace = sidebarWorkspace(key);
  if (!workspace) return;
  openCatalogDialog({ kind: 'new-session', workspace: { key, revision: workspace.revision, name: workspace.name, invalid: workspace.invalid } });
}
function requestRenameWorkspace(key: string) {
  const workspace = sidebarWorkspace(key);
  if (workspace) openCatalogDialog({ kind: 'rename-workspace', workspace: { key, revision: workspace.revision, name: workspace.name } });
}
function requestDeleteWorkspace(key: string) {
  const workspace = sidebarWorkspace(key);
  if (workspace)
    openCatalogDialog({
      kind: 'delete-workspace',
      workspace: { key, revision: workspace.revision, name: workspace.name, lifecycle: workspace.lifecycle },
    });
}
function requestRenameAgent(workspaceKeyValue: string, agentId: string) {
  const workspace = sidebarWorkspace(workspaceKeyValue);
  const agent = workspace?.agents.find((item) => item.id === agentId);
  if (workspace && agent)
    openCatalogDialog({
      kind: 'rename-agent',
      workspace: { key: workspace.key, revision: workspace.revision, name: workspace.name },
      agent: { id: agent.id, name: agent.name },
    });
}
function requestDeleteAgent(workspaceKeyValue: string, agentId: string) {
  const workspace = sidebarWorkspace(workspaceKeyValue);
  const agent = workspace?.agents.find((item) => item.id === agentId);
  if (workspace && agent)
    openCatalogDialog({
      kind: 'delete-agent',
      workspace: { key: workspace.key, revision: workspace.revision, name: workspace.name },
      agent: { id: agent.id, name: agent.name },
    });
}
/** The dialog never shows optimistic success; a daemon rejection stays in the dialog. */
async function submitCatalogDialog(value: DialogValue) {
  catalogDialogBusy.value = true;
  try {
    if (value.kind === 'new-workspace') {
      await store.createWorkspace({ name: value.name, projectRoot: value.projectRoot });
    } else if (value.kind === 'new-session') {
      await store.addMember(value.name.trim(), value.driver.trim(), { key: value.key, revision: value.revision });
    } else if (value.kind === 'rename-workspace') {
      await store.renameWorkspace(value.key, value.name, value.revision);
    } else if (value.kind === 'rename-agent') {
      await store.renameMember(value.agent, value.name, { key: value.key, revision: value.revision });
    } else if (value.kind === 'delete-workspace') {
      await store.deleteWorkspace(value.key, value.revision);
    } else if (value.kind === 'delete-agent') {
      await store.deleteMember(value.agent, { key: value.key, revision: value.revision });
    }
    catalogDialog.value = null;
  } catch (error) {
    catalogDialogEl.value?.showError(error instanceof Error ? error.message : String(error));
  } finally {
    catalogDialogBusy.value = false;
  }
}
const displayedLogs = computed(() => {
  if (logFilter.value === 'errors') {
    return logs.value.filter((entry) => entry.level === 'ERROR' || entry.level === 'WARN');
  }
  return logs.value;
});

const selectedAgentFailure = computed(() => {
  const workspace = selectedWorkspace.value;
  const agent = selectedAgent.value;
  if (!workspace || !agent) return null;
  return lastFailures.value.get(`${workspaceKey(workspace)}:${agent}`) ?? null;
});

const connectionLabel = computed(() => {
  if (connectionStatus.value === 'online') return 'Connected';
  if (connectionStatus.value === 'connecting') return 'Connecting';
  return 'Offline';
});

const toolResources = computed(() =>
  resourceVisibility.value.filter((entry) => entry.resource.startsWith('tool:')),
);

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
  const input = tokenUsage.value.input;
  const output = tokenUsage.value.output;
  const cached = tokenUsage.value.cached;
  const hitRate = input > 0 ? cached / input : 0;
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
  const used = contextTokens.value;
  const maximum = contextWindowTokens.value;
  const known =
    used !== null &&
    Number.isFinite(used) &&
    used >= 0 &&
    maximum !== null &&
    Number.isFinite(maximum) &&
    maximum > 0;
  const ratio = known ? used / maximum : 0;
  return {
    ratio,
    dashOffset: 44 * (1 - Math.min(ratio, 1)),
    label: known ? `${(ratio * 100).toFixed(1)}%` : '—',
    title: known
      ? `Context window: ${used.toLocaleString()} / ${maximum.toLocaleString()} tokens (${(ratio * 100).toFixed(1)}%). Based on the latest assistant usage.`
      : 'Context window usage unavailable',
  };
});

const currentChannelLabel = computed(() => {
  const workspace = selectedWorkspace.value;
  if (!workspace) return 'Select a workspace';
  if (selectedAgent.value) return resourceName(selectedAgent.value);
  return workspace.agents.length ? 'Choose an agent' : 'No agents defined';
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

function toolArguments(value: string) {
  return value;
}

function logLevelClass(log: { level: string }) {
  return `log-level--${log.level.toLowerCase()}`;
}

function closeToolsOnOutsideClick(event: MouseEvent) {
  const target = event.target;
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
  if ((!text && !draftImages.value.length) || !canEditComposer.value || importingImages.value) return;
  const generation = draftGeneration;
  submittingMessage.value = true;
  composerError.value = '';
  try {
    await store.sendMessage(text, wireImages(draftImages.value), (stage) => {
      if (generation === draftGeneration) sendStage.value = stage;
    });
    if (generation !== draftGeneration) return;
    draft.value = '';
    draftImages.value = [];
    nextTick(scrollMessages);
  } catch (error) {
    if (generation === draftGeneration) composerError.value = error instanceof Error ? error.message : 'Send failed';
  } finally {
    if (generation === draftGeneration) {
      submittingMessage.value = false;
      sendStage.value = null;
    }
  }
}

async function runAgentAction(action: () => Promise<unknown>) {
  try {
    await action();
  } catch (error) {
    connectionError.value = error instanceof Error ? error.message : String(error);
  }
}

function toggleResourceVisibility(resource: string, visible: boolean) {
  void runAgentAction(() => store.setResourceVisibility(resource, visible));
}

function abortTurn() {
  void runAgentAction(() => store.abortTurn());
}

const NO_SANDBOX = '';
const sandboxChoices = computed(() => [
  {
    value: NO_SANDBOX,
    label: 'None',
    hint: 'No sandbox: tools run with the daemon user permissions',
  },
  ...sandboxOptions.value.options.map((resource) => ({
    value: resource,
    label: resourceName(resource),
    hint: resource,
  })),
]);
const sandboxSelection = computed(() => sandboxOptions.value.activated[0] ?? NO_SANDBOX);

function selectSandbox(value: string) {
  void runAgentAction(() => store.setSandbox(value === NO_SANDBOX ? null : value));
}

function selectWorkspace(key: string) {
  store.selectWorkspace(key);
  sidebarOpen.value = false;
  nextTick(scrollMessages);
}

function selectMember(agent: string | null, key?: string) {
  store.selectAgent(agent, key);
  sidebarOpen.value = false;
  nextTick(scrollMessages);
}

function openSettings() {
  endpointDraft.value = endpoint.value;
  contextPathDraft.value = driverContextPath.value;
  formError.value = '';
  settingsOpen.value = true;
}

function saveSettings() {
  try {
    driverContextPath.value = contextPathDraft.value.trim();
    if (store.connect(endpointDraft.value)) settingsOpen.value = false;
    else formError.value = connectionError.value;
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
    if (target.closest('.tool-manager-panel, .image-row')) return;
    // Keep native scrolling inside an overflowing textarea draft.
    if (target instanceof HTMLTextAreaElement && target.scrollHeight > target.clientHeight) {
      return;
    }
  }

  event.preventDefault();
  list.scrollTop += event.deltaY;
}

function formatTime(timestamp: number) {
  return formatRecordTime(timestamp);
}

function formatLogTime(timestamp: number) {
  return formatRecordTime(timestamp, true);
}

function roleLabel(message: ConversationEntry) {
  if (message.role === 'user') return 'You';
  if (message.role === 'assistant') return resourceName(message.agent);
  if (message.role === 'tool') return 'Tool response';
  if (message.role === 'error') return 'Agent error';
  if (message.role === 'record') return 'Note';
  return 'System';
}

function isMcLRecord(message: ConversationEntry) {
  return message.role === 'record' && message.kind === 'mcl';
}

const mclRecords = computed(() => visibleMessages.value.filter(isMcLRecord));
const conversationMessages = computed(() =>
  visibleMessages.value.filter((message) => !isMcLRecord(message)),
);
</script>

<template>
  <div class="app-shell">
    <div v-if="sidebarOpen" class="mobile-scrim" @click="sidebarOpen = false"></div>
    <aside :class="['workspace-sidebar', { 'workspace-sidebar--open': sidebarOpen }]">
      <div class="brand-row">
        <div class="brand-mark">
          <Command :size="16" :stroke-width="2.4" />
        </div>
        <div class="brand-copy">
          <strong>margatroid</strong>
          <span>{{ connectionLabel }}</span>
        </div>
        <button class="icon-button sidebar-close" title="Close navigation" @click="sidebarOpen = false">
          <X :size="17" />
        </button>
      </div>

      <button class="connection-strip" title="Open connection settings" @click="openSettings">
        <span :class="['status-dot', `status-dot--${connectionStatus}`]" aria-hidden="true"></span>
        <span class="connection-copy">
          <strong>{{ connectionLabel }}</strong>
          <small>{{ endpoint.replace(/^wss?:\/\//, '') }}</small>
        </span>
        <Settings2 :size="15" />
      </button>

      <div class="sidebar-heading">
        <span>Workspaces</span>
        <button
          class="heading-action"
          type="button"
          :disabled="workspaceOperationPending || connectionStatus !== 'online'"
          title="Rescan the workspace catalog"
          @click="runAgentAction(() => store.refreshCatalog())"
        >
          Refresh
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
          @new-workspace="openCatalogDialog({ kind: 'new-workspace' })"
          @new-session="requestNewSession"
          @rename-workspace="requestRenameWorkspace"
          @delete-workspace="requestDeleteWorkspace"
          @rename-agent="requestRenameAgent"
          @delete-agent="requestDeleteAgent"
        />
      </nav>

      <div v-if="selectedCatalogEntry" class="workspace-controls" aria-label="Selected workspace controls">
        <span class="tree-name" :title="selectedCatalogEntry.name">{{ selectedCatalogEntry.name }}</span>
        <button type="button" :disabled="workspaceOperationPending || connectionStatus !== 'online' || !!selectedCatalogEntry.error || selectedCatalogEntry.lifecycle !== 'stopped'" @click="runAgentAction(() => store.startWorkspace())">Start</button>
        <button type="button" :disabled="workspaceOperationPending || connectionStatus !== 'online' || selectedCatalogEntry.lifecycle === 'stopped'" @click="runAgentAction(() => store.stopWorkspace())">Stop</button>
      </div>
      <div v-if="selectedCatalogEntry || workspaceNotice" class="sidebar-footnote" role="status">
        <template v-if="selectedCatalogEntry?.error">
          Invalid definition at {{ selectedCatalogEntry.path }}. Fix it, then Refresh.
        </template>
        <template v-else-if="workspaceNotice">{{ workspaceNotice }}</template>
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
          <button class="icon-button" :class="{ 'icon-button--active': activityOpen }" title="Toggle activity"
            @click="activityOpen = !activityOpen">
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
        <div v-else-if="!conversationMessages.length" class="empty-state empty-state--conversation">
          <div class="empty-icon">
            <MessageSquare :size="22" />
          </div>
          <strong>{{ selectedAgentName }}</strong>
          <span v-if="selectedAgentState?.status === 'creating'">Agent is starting.</span>
          <span v-else-if="!selectedWorkspace.agents.length">No agents defined. Add an agent to begin.</span>
          <span v-else-if="!selectedAgent">Choose an agent from Members.</span>
          <span v-else-if="!selectedAgentReady">Agent is not running. Start the workspace or inspect runtime errors.</span>
          <span v-else>Conversation is ready.</span>
          <span v-if="selectedAgentFailure" class="agent-failure">
            Last error: {{ selectedAgentFailure.message }}
          </span>
        </div>

        <template v-for="message in conversationMessages" :key="message.key">
          <article :class="[
            'message-row',
            `message-row--${message.role}`,
            { 'message-row--failed': message.failed },
          ]">
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
              <div v-if="message.images?.length" class="image-row history-image-row" aria-label="Message images">
                <img v-for="(image, index) in message.images" :key="index" :src="imageSource(image)" :alt="`Image ${index + 1}`" loading="lazy" />
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
              <div v-else-if="message.content" class="message-content" v-html="renderMarkdown(message.content)"></div>
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
        </template>

        <div v-if="connectionError" class="inline-error">
          <CloudOff :size="16" />
          <span>{{ connectionError }}</span>
          <button class="text-button" @click="openSettings">Configure</button>
        </div>
      </section>

      <footer ref="composerFooter" class="composer" @wheel="handleComposerWheel">
        <form class="composer-box" @submit.prevent="submitMessage" @dragover.prevent @drop.prevent="dropImages">
          <input ref="imagePicker" class="image-picker" type="file" :accept="IMAGE_ACCEPT" multiple @change="pickImages" />
          <div v-if="draftImages.length" class="image-row draft-image-row" aria-label="Attached images">
            <div v-for="image in draftImages" :key="image.id" class="draft-image">
              <img :src="imageSource(image)" :alt="image.name" :title="image.name" />
              <button type="button" :disabled="submittingMessage" :aria-label="`Remove ${image.name}`" @click="draftImages = draftImages.filter((item) => item.id !== image.id)"><X :size="14" /></button>
            </div>
          </div>
          <div class="composer-input-row">
            <textarea ref="draftInput" v-model="draft"
              :disabled="!canEditComposer" rows="1"
              :placeholder="!selectedWorkspace
                  ? 'Select a workspace...'
                  : selectedAgentReady
                    ? selectedAgentWorking
                      ? `${selectedAgentName} is working...`
                      : `Message ${selectedAgentName}...`
                    : 'Agent is not ready...'
                " @keydown.enter.exact.prevent="submitMessage"></textarea>
          </div>
          <div v-if="composerError" class="composer-error" role="alert">{{ composerError }}</div>
          <div class="composer-actions">
            <div class="upload-control">
              <button class="icon-button" type="button" title="Add image" aria-label="Add image" :aria-expanded="uploadMenuOpen" :disabled="!canEditComposer || importingImages" @click="uploadMenuOpen = !uploadMenuOpen"><Plus :size="19" /></button>
              <div v-if="uploadMenuOpen" class="upload-menu">
                <button type="button" @click="openImagePicker">上传图片 / Upload image</button>
              </div>
            </div>
            <span v-if="importingImages" class="image-import-label">Importing images…</span>
            <span v-if="submittingMessage" class="image-import-label" role="status">{{ sendStage === 'waking' ? 'Message injected; waiting for wake acknowledgement…' : 'Sending; waiting for daemon acknowledgement…' }}</span>
            <span v-if="abortPending" class="abort-pending-label">Abort pending</span>
            <button v-if="selectedAgentWorking" class="send-button send-button--stop" type="button" :title="canAbortTurn
                ? 'Stop current turn'
                : abortPending ? 'Abort pending' : 'No abortable turn'
              " :disabled="!canAbortTurn ||
                !selectedWorkspace ||
                connectionStatus !== 'online' ||
                !selectedAgentReady
                " @click="abortTurn">
              <Square :size="14" :stroke-width="0" fill="currentColor" />
            </button>
            <button v-else class="send-button" type="submit" title="Send message" :disabled="(!draft.trim() && !draftImages.length) ||
              importingImages || submittingMessage ||
              !selectedWorkspace ||
              connectionStatus !== 'online' ||
              !selectedAgentIdle
              ">
              <Send :size="18" />
            </button>
          </div>
        </form>
        <div v-if="selectedWorkspace" ref="toolManager" class="tool-manager">
          <button class="tool-manager-trigger" type="button" :aria-expanded="toolsOpen"
            aria-controls="tool-manager-panel" title="Manage agent tools" @click="toolsOpen = !toolsOpen">
            <ChevronRight :class="['tool-manager-chevron', { 'is-open': toolsOpen }]" :size="13" />
            <span class="tool-manager-empty">Resources Panel</span>
          </button>

          <div class="token-usage" aria-label="Agent token usage">
            <span v-for="item in tokenUsageItems" :key="item.label" class="token-usage-item" :title="item.title">
              <span>{{ item.label }}</span>
              <strong>{{ item.value }}</strong>
            </span>
            <span class="context-window-usage" role="img" :aria-label="contextWindowUsage.title"
              :title="contextWindowUsage.title">
              <svg viewBox="0 0 18 18" aria-hidden="true">
                <circle class="context-window-track" cx="9" cy="9" r="7" />
                <circle class="context-window-value" cx="9" cy="9" r="7"
                  :style="{ strokeDashoffset: contextWindowUsage.dashOffset }" />
              </svg>
              <span class="context-window-percentage">{{ contextWindowUsage.label }}</span>
            </span>
          </div>

          <div v-if="toolsOpen" id="tool-manager-panel" class="tool-manager-panel">
            <section class="tool-resource-group">
              <div class="tool-resource-heading">
                <span>Sandbox</span>
                <span>{{ sandboxOptions.options.length }}</span>
              </div>
              <div class="tool-resource-list">
                <div v-for="choice in sandboxChoices" :key="choice.value || 'none'" class="tool-resource-row"
                  :title="choice.hint">
                  <ShieldCheck v-if="choice.value" :size="14" />
                  <ShieldOff v-else :size="14" />
                  <code :title="choice.hint">{{ choice.label }}</code>
                  <button class="tool-row-action" type="button" :title="sandboxSelection === choice.value ? 'Active sandbox' : 'Use this sandbox'
                    " :aria-label="sandboxSelection === choice.value ? 'Active sandbox' : 'Use this sandbox'
                      " :disabled="connectionStatus !== 'online' || !selectedAgentReady"
                    @click="selectSandbox(choice.value)">
                    <CircleDot v-if="sandboxSelection === choice.value" :size="14" />
                    <Circle v-else :size="14" />
                  </button>
                </div>
              </div>
            </section>

            <section class="tool-resource-group">
              <div class="tool-resource-heading">
                <span>Tools</span>
                <span>{{ toolResources.length }}</span>
              </div>
              <div v-if="toolResources.length" class="tool-resource-list">
                <div v-for="entry in toolResources" :key="entry.resource" class="tool-resource-row">
                  <span :class="['resource-state-dot', { 'is-active': entry.visible }]" />
                  <code :title="entry.resource">{{ resourceName(entry.resource) }}</code>
                  <button v-if="entry.default" class="tool-row-action" type="button"
                    :title="entry.visible ? 'Remove visibility' : 'Inject visibility'"
                    :aria-label="entry.visible ? 'Remove visibility' : 'Inject visibility'"
                    :disabled="connectionStatus !== 'online' || !selectedAgentReady"
                    @click="toggleResourceVisibility(entry.resource, !entry.visible)">
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
        <button class="icon-button" title="Close panel" @click="activityOpen = false">
          <X :size="17" />
        </button>
      </div>
      <div class="activity-tabs" role="tablist" aria-label="Audit panel">
        <button type="button" role="tab" :aria-selected="activityTab === 'activity'" :class="{ active: activityTab === 'activity' }" @click="activityTab = 'activity'">Activity</button>
        <button type="button" role="tab" :aria-selected="activityTab === 'mcl'" :class="{ active: activityTab === 'mcl' }" @click="activityTab = 'mcl'">MCL <span>{{ mclRecords.length }}</span></button>
      </div>
      <div v-if="activityTab === 'mcl'" class="activity-list" role="tabpanel" aria-label="MCL audit">
        <p v-if="!mclRecords.length" class="activity-empty">No MCL records for the selected agent</p>
        <details v-for="record in mclRecords" :key="record.key" class="mcl-audit-entry">
          <summary>{{ record.content || 'MCL command' }} · {{ formatTime(record.timestamp) }}</summary>
          <pre>{{ metadataText(record.payload) }}</pre>
        </details>
      </div>
      <div v-if="activityTab === 'activity'" class="activity-filter" role="group" aria-label="Filter activity">
        <button type="button" :class="{ active: logFilter === 'all' }" @click="logFilter = 'all'">
          All <span>{{ logs.length }}</span>
        </button>
        <button type="button" :class="{ active: logFilter === 'errors' }" @click="logFilter = 'errors'">
          Errors <span>{{ logs.filter((log) => log.level === 'ERROR' || log.level === 'WARN').length }}</span>
        </button>
      </div>
      <div v-if="activityTab === 'activity'" class="activity-list">
        <div v-if="!displayedLogs.length" class="activity-empty">
          <Activity :size="18" />
          <span>No runtime events</span>
        </div>
        <div v-for="log in displayedLogs" :key="log.key" class="log-entry">
          <span :class="['log-level', logLevelClass(log)]">{{ log.level }}</span>
          <div class="log-copy">
            <p>{{ log.message }}</p>
            <small>{{ log.target }} · {{ formatLogTime(log.timestamp) }}</small>
            <code v-for="field in log.fields" :key="field.name">{{ field.name }}={{ field.value }}</code>
          </div>
        </div>
      </div>
      <button v-if="activityTab === 'activity'" class="activity-clear" type="button" @click="store.clearLogs">Clear activity</button>
    </aside>

    <CatalogDialog
      v-if="catalogDialog"
      ref="catalogDialogEl"
      :request="catalogDialog"
      :drivers="drivers"
      :driver-errors="store.driverCatalogErrors"
      :query-paths="store.queryPaths"
      :refresh-drivers="() => store.queryDrivers(true)"
      :busy="catalogDialogBusy || workspaceOperationPending"
      :offline="connectionStatus !== 'online'"
      @close="!catalogDialogBusy && (catalogDialog = null)"
      @submit="submitCatalogDialog"
    />
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
        <input id="endpoint" v-model="endpointDraft" class="field-input" placeholder="ws://127.0.0.1:3939/ws"
          @keydown.enter="saveSettings" />
        <label class="field-label" for="context-path">Driver context path</label>
        <input id="context-path" v-model="contextPathDraft" class="field-input"
          placeholder="Path defined by your driver" @keydown.enter="saveSettings" />
        <p class="field-label">
          Driver-defined destination for messages. Change only if your driver uses a different path.
        </p>
        <p v-if="formError" class="form-error">{{ formError }}</p>
        <div class="dialog-actions">
          <button class="secondary-button" @click="settingsOpen = false">Cancel</button>
          <button class="primary-button" @click="saveSettings">
            <Wifi :size="16" /> Connect
          </button>
        </div>
      </section>
    </div>
  </div>
</template>
