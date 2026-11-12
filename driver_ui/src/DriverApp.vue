<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue';

interface DriverBootstrap {
  endpoint: string;
  address: string;
  contextPath?: string;
}
const driverBoot: DriverBootstrap | undefined = (
  window as unknown as { __MARGATROID_DRIVER__?: DriverBootstrap }
).__MARGATROID_DRIVER__;
const driverMode = !!driverBoot;
import { storeToRefs } from 'pinia';
import { isRecord } from '@/types/protocol';
import {
  Activity,
  Check,
  ChevronRight,
  Copy,
  Circle,
  CircleDot,
  ShieldCheck,
  ShieldOff,
  CloudOff,
  Command,
  Menu,
  MessageSquare,
  PanelRight,
  Plus,
  Send,
  Square,
  Settings2,
  UserRound,
  UsersRound,
  Wifi,
  Wrench,
  X,
} from 'lucide-vue-next';

import {
  useWorkbenchStore,
  resourceName,
  workspaceKey,
  type ConversationEntry,
} from '@/stores/workbench';
import SidebarTree from '@/components/SidebarTree.vue';
import { formatToolPayload, inlineToolPayload } from '@/lib/format';
import { renderMarkdown } from '@/lib/markdown';
import { formatRecordTime } from '@/lib/time';
import {
  IMAGE_ACCEPT,
  importImage,
  imageSource,
  validateImageBudget,
  wireImages,
  metadataText,
  type DraftImage,
} from '@/lib/images';

const store = useWorkbenchStore();
const {
  endpoint,
  driverContextPath,
  tokenUsage,
  contextTokens,
  contextWindowTokens,
  connectionStatus,
  stateReady,
  connectionError,
  workspaces,
  selectedDefinition,
  selectedRuntimeWorkspace,
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
  toolOptions,
  sandboxOptions,
  resourceWriteInFlight,
  visibleMessages,
  selectedMclEntries,
  selectedSessionLogs,
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
const composerRoute = computed(
  () =>
    `${endpoint.value}:${selectedWorkspace.value?.path}:${selectedAgent.value}`,
);
watch(
  composerRoute,
  () => {
    draftGeneration++;
    draft.value = '';
    draftImages.value = [];
    composerError.value = '';
    importingImages.value = false;
    submittingMessage.value = false;
    sendStage.value = null;
    uploadMenuOpen.value = false;
    if (imagePicker.value) imagePicker.value.value = '';
  },
  { flush: 'sync' },
);
const canEditComposer = computed(
  () =>
    !!selectedWorkspace.value &&
    connectionStatus.value === 'online' &&
    stateReady.value &&
    selectedAgentIdle.value &&
    !submittingMessage.value,
);
async function importFiles(files: File[]) {
  if (!canEditComposer.value || importingImages.value || !files.length) return;
  const generation = draftGeneration;
  importingImages.value = true;
  composerError.value = '';
  try {
    validateImageBudget([
      ...draftImages.value.map((image) => image.size),
      ...files.map((file) => file.size),
    ]);
    const imported: DraftImage[] = [];
    for (const file of files) imported.push(await importImage(file));
    if (generation === draftGeneration) draftImages.value.push(...imported);
  } catch (error) {
    if (generation === draftGeneration)
      composerError.value = error instanceof Error ? error.message : 'Image import failed';
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
const uploadControl = ref<HTMLElement | null>(null);
const toolsOpen = ref(false);
const sidebarOpen = ref(false);
const activityOpen = ref(false);
const activityTab = ref<'activity' | 'mcl'>('activity');
const settingsOpen = ref(false);
const logFilter = ref<'all' | 'errors'>('all');
const endpointDraft = ref(endpoint.value);
const contextPathDraft = ref(driverContextPath.value);
const formError = ref('');
const activityLogs = computed(() => [...logs.value, ...selectedSessionLogs.value]);
watch(
  [activityOpen, activityTab, selectedAgent],
  () => {
    if (activityOpen.value && selectedAgentReady.value)
      store.queryRecords(activityTab.value === 'mcl' ? 'mcl' : 'log');
  },
);
const displayedLogs = computed(() => {
  if (logFilter.value === 'errors') {
    return activityLogs.value.filter((entry) => entry.level === 'ERROR' || entry.level === 'WARN');
  }
  return activityLogs.value;
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

const selectedModel = computed(() => selectedAgentState.value?.model ?? null);
const modelTitle = computed(() => {
  const model = selectedModel.value;
  if (!model) return '';
  const details = [
    `${model.model} · ${model.provider}/${model.api_type}`,
    `Context window: ${model.context_window_tokens.toLocaleString()} tokens`,
    model.thinking ? `Thinking: ${model.thinking}` : '',
    model.reasoning_effort ? `Reasoning effort: ${model.reasoning_effort}` : '',
  ].filter(Boolean);
  return details.join('\n');
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
  return selectedDefinition.value?.cache.agents.length
    ? 'Choose an agent'
    : 'No members configured';
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

function logLevelClass(log: { level: string }) {
  return `log-level--${log.level.toLowerCase()}`;
}

function closeFloatingOnOutsideClick(event: MouseEvent) {
  const target = event.target;
  if (!(target instanceof Node)) return;
  if (toolsOpen.value && toolManager.value && !toolManager.value.contains(target))
    toolsOpen.value = false;
  if (uploadMenuOpen.value && uploadControl.value && !uploadControl.value.contains(target))
    uploadMenuOpen.value = false;
}

function closeOverlaysOnEscape(event: KeyboardEvent) {
  if (event.key !== 'Escape') return;
  settingsOpen.value = false;
  activityOpen.value = false;
  toolsOpen.value = false;
  uploadMenuOpen.value = false;
  sidebarOpen.value = false;
}

function onComposerEnter(event: KeyboardEvent) {
  // Enter confirms an IME candidate instead of sending while composing.
  if (event.isComposing || event.keyCode === 229) return;
  event.preventDefault();
  void submitMessage();
}

let driverSelectionWatch: (() => void) | undefined;
function selectDriverAgent() {
  if (!driverBoot) return;
  const workspace = store.sidebarTree.workspaces.find((entry) =>
    entry.agents.some((agent) => agent.id === driverBoot.address),
  );
  if (!workspace) return;
  if (store.selectedAgent !== driverBoot.address || store.selectedWorkspaceKey !== workspace.path)
    store.selectAgent(driverBoot.address, workspace.path);
}
onMounted(() => {
  if (driverBoot) {
    store.endpoint = driverBoot.endpoint;
    endpointDraft.value = driverBoot.endpoint;
    if (driverBoot.contextPath) store.driverContextPath = driverBoot.contextPath;
    driverSelectionWatch = watch(() => store.sidebarTree, selectDriverAgent, {
      deep: true,
      immediate: true,
    });
  } else if (!selectedWorkspace.value && workspaces.value[0]) {
    store.selectWorkspace(workspaces.value[0].path);
  }
  store.connect();
  document.addEventListener('click', closeFloatingOnOutsideClick);
  document.addEventListener('keydown', closeOverlaysOnEscape);

  composerObserver = new ResizeObserver(() => {
    const pane = conversationPane.value;
    const footer = composerFooter.value;
    if (!pane || !footer) return;
    pane.style.setProperty('--mg-composer-height', `${footer.offsetHeight}px`);
  });
  if (composerFooter.value) composerObserver.observe(composerFooter.value);
});

onBeforeUnmount(() => {
  driverSelectionWatch?.();
  document.removeEventListener('click', closeFloatingOnOutsideClick);
  document.removeEventListener('keydown', closeOverlaysOnEscape);
  clearTimeout(copiedTimer);
  clearTimeout(olderTimer);
  composerObserver?.disconnect();
  store.disconnect();
});

async function submitMessage() {
  const text = draft.value.trim();
  if ((!text && !draftImages.value.length) || !canEditComposer.value || importingImages.value)
    return;
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
    if (generation === draftGeneration)
      composerError.value = error instanceof Error ? error.message : 'Send failed';
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
    const message = error instanceof Error ? error.message : String(error);
    if (connectionStatus.value === 'online') composerError.value = message;
    else connectionError.value = message;
  }
}

async function runWorkspaceAction(action: () => Promise<unknown>) {
  try {
    await action();
    workspaceNotice.value = '';
  } catch (error) {
    workspaceNotice.value = error instanceof Error ? error.message : String(error);
  }
}

function startWorkspace(path: string) {
  void runWorkspaceAction(() => store.startWorkspace(path));
}

function restartWorkspace(path: string) {
  void runWorkspaceAction(() => store.restartWorkspace(path));
}

function stopWorkspace(address: string) {
  void runWorkspaceAction(() => store.stopWorkspace(address));
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
    label: resource,
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

const olderPending = ref(false);
let olderAnchor: { height: number; top: number } | null = null;
let olderTimer: ReturnType<typeof setTimeout> | undefined;

function loadEarlierMessages() {
  const list = messageList.value;
  if (!list || olderPending.value) return;
  olderAnchor = { height: list.scrollHeight, top: list.scrollTop };
  olderPending.value = true;
  store.loadOlderHistory();
  clearTimeout(olderTimer);
  olderTimer = setTimeout(() => {
    olderPending.value = false;
    olderAnchor = null;
  }, 1500);
}

function onMessageScroll() {
  const list = messageList.value;
  if (!list || olderPending.value || list.scrollTop > 96) return;
  loadEarlierMessages();
}

watch(
  () => conversationMessages.value.length,
  async (next, previous) => {
    if (!olderAnchor || next <= previous) return;
    await nextTick();
    const list = messageList.value;
    if (list) list.scrollTop = list.scrollHeight - olderAnchor.height + olderAnchor.top;
    olderPending.value = false;
    olderAnchor = null;
    clearTimeout(olderTimer);
  },
);

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

const copiedKey = ref('');
let copiedTimer: ReturnType<typeof setTimeout> | undefined;

async function copyText(value: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(value);
    return true;
  } catch {
    // Clipboard API needs a secure context; keep a legacy fallback.
    const area = document.createElement('textarea');
    area.value = value;
    area.setAttribute('readonly', '');
    area.style.position = 'fixed';
    area.style.opacity = '0';
    document.body.appendChild(area);
    area.select();
    const copied = document.execCommand('copy');
    area.remove();
    return copied;
  }
}

async function copyMessage(message: ConversationEntry) {
  if (!message.content) return;
  if (!(await copyText(message.content))) {
    composerError.value = 'Could not copy the message';
    return;
  }
  copiedKey.value = message.key;
  clearTimeout(copiedTimer);
  copiedTimer = setTimeout(() => {
    if (copiedKey.value === message.key) copiedKey.value = '';
  }, 1200);
}

function toolResult(message: ConversationEntry, id: string) {
  return message.toolResults?.get(id) ?? null;
}

function toolState(message: ConversationEntry, id: string): 'pending' | 'done' | 'failed' {
  const result = toolResult(message, id);
  if (!result) return 'pending';
  return result.failed ? 'failed' : 'done';
}

// Tool name and arguments share one text run: same font, colour, size and
// baseline, separated by two spaces, a dot and two spaces.
function toolCallLabel(tool: { tool_name: string; arguments: unknown }): string {
  return `${tool.tool_name}\u00a0\u00a0·\u00a0\u00a0${inlineToolPayload(tool.arguments ?? {})}`;
}

function isMcLRecord(message: ConversationEntry) {
  return message.role === 'record' && message.kind === 'mcl';
}

const mclRecords = computed(() => selectedMclEntries.value);
const conversationMessages = computed(() =>
  visibleMessages.value.filter((message) => !isMcLRecord(message)),
);

function stopReason(message: ConversationEntry): string | null {
  const payload = message.payload;
  if (!isRecord(payload)) return null;
  if (typeof payload.stop_reason === 'string') return payload.stop_reason;
  if (isRecord(payload.message) && typeof payload.message.stop_reason === 'string')
    return payload.message.stop_reason;
  return null;
}

// Only a user message and a completed assistant reply carry timestamp and copy.
function showsMeta(index: number): boolean {
  const message = conversationMessages.value[index];
  if (!message) return false;
  if (message.role === 'user') return true;
  return message.role === 'assistant' && stopReason(message) === 'completed';
}

// The transcript is one flat block flow. A message is not one visual box: its
// thinking, text and tool calls each become sibling blocks, so the gap between
// them never depends on which message they came from.
type RowKind =
  'images' | 'thinking' | 'content' | 'error' | 'tool' | 'toolResult' | 'recordData' | 'meta';
interface ConversationRow {
  key: string;
  role: ConversationEntry['role'];
  kind: RowKind;
  message: ConversationEntry;
  tool?: ConversationEntry['toolCalls'][number];
}
const conversationRows = computed<ConversationRow[]>(() => {
  const rows: ConversationRow[] = [];
  conversationMessages.value.forEach((message, index) => {
    const push = (kind: RowKind, tool?: ConversationEntry['toolCalls'][number]) =>
      rows.push({
        key: `${message.key}:${kind}${tool ? `:${tool.id}` : ''}`,
        role: message.role,
        kind,
        message,
        tool,
      });
    if (message.role === 'user') {
      if (message.images?.length) push('images');
      if (message.content) push('content');
      if (showsMeta(index)) push('meta');
      return;
    }
    if (message.role === 'assistant') {
      if (message.thinking) push('thinking');
      if (message.content) push('content');
      for (const tool of message.toolCalls) push('tool', tool);
      if (showsMeta(index)) push('meta');
      return;
    }
    if (message.role === 'tool') {
      push('toolResult');
      return;
    }
    if (message.role === 'record') {
      if (message.content) push('content');
      if (message.payload !== null) push('recordData');
      return;
    }
    if (message.content) push(message.role === 'error' ? 'error' : 'content');
  });
  return rows;
});
</script>

<template>
  <div :class="['app-shell', { 'driver-mode': driverMode }]">
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
        <button
          class="icon-button sidebar-close"
          title="Close navigation"
          @click="sidebarOpen = false"
        >
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

      <div class="sidebar-heading"><span>Workspaces</span></div>

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

      <section
        ref="messageList"
        class="message-list"
        aria-live="polite"
        @scroll="onMessageScroll"
      >
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
          <span v-if="selectedAgentState?.status === 'starting'">Agent is starting.</span>
          <span v-else-if="!selectedRuntimeWorkspace"
            >Workspace is stopped. Start it to run its configured members.</span
          >
          <span v-else-if="!selectedDefinition?.cache.agents.length">No members configured.</span>
          <span v-else-if="!selectedAgent">Choose an agent in the sidebar.</span>
          <span v-else-if="!selectedAgentReady"
            >Agent is not running. Start the workspace or inspect runtime errors.</span
          >
          <span v-else>Conversation is ready.</span>
          <span v-if="selectedAgentFailure" class="agent-failure">
            Last error: {{ selectedAgentFailure.message }}
          </span>
        </div>

        <div v-if="conversationRows.length" class="message-flow">
          <template v-for="row in conversationRows" :key="row.key">
            <article
              :class="['message-row', `message-row--${row.role}`, `message-kind--${row.kind}`]"
            >
              <div class="message-body">
                <div
                  v-if="row.kind === 'images'"
                  class="image-row history-image-row"
                  aria-label="Message images"
                >
                  <img
                    v-for="(image, index) in row.message.images ?? []"
                    :key="index"
                    :src="imageSource(image)"
                    :alt="`Image ${index + 1}`"
                    loading="lazy"
                  />
                </div>
                <details
                  v-else-if="row.kind === 'thinking'"
                  class="message-disclosure message-thinking"
                >
                  <summary>Thinking</summary>
                  <p>{{ row.message.thinking }}</p>
                </details>
                <div
                  v-else-if="row.kind === 'error'"
                  class="message-content message-content--plain"
                >
                  {{ row.message.content }}
                </div>
                <div
                  v-else-if="row.kind === 'content'"
                  class="message-content"
                  v-html="renderMarkdown(row.message.content)"
                ></div>
                <details v-else-if="row.kind === 'recordData'" class="message-disclosure">
                  <summary>Record data</summary>
                  <pre class="message-pre">{{ metadataText(row.message.payload) }}</pre>
                </details>
                <details
                  v-else-if="row.kind === 'toolResult'"
                  :class="[
                    'message-disclosure',
                    'tool-call-row',
                    row.message.failed ? 'tool-call-row--failed' : 'tool-call-row--done',
                  ]"
                >
                  <summary>
                    <Wrench :size="14" />
                    <span>{{ row.message.toolName || 'tool' }}</span>
                  </summary>
                  <pre v-if="row.message.content" class="message-pre">{{
                    formatToolPayload(row.message.content)
                  }}</pre>
                  <div
                    v-if="row.message.images?.length"
                    class="image-row history-image-row"
                    aria-label="Tool result images"
                  >
                    <img
                      v-for="(image, index) in row.message.images"
                      :key="index"
                      :src="imageSource(image)"
                      :alt="`Tool image ${index + 1}`"
                      loading="lazy"
                    />
                  </div>
                </details>
                <details
                  v-else-if="row.kind === 'tool' && row.tool"
                  :class="[
                    'message-disclosure',
                    'tool-call-row',
                    `tool-call-row--${toolState(row.message, row.tool.id)}`,
                  ]"
                >
                  <summary>
                    <Wrench :size="14" />
                    <span>{{ toolCallLabel(row.tool) }}</span>
                  </summary>
                  <pre class="message-pre">{{ formatToolPayload(row.tool.arguments ?? {}) }}</pre>
                  <template v-if="toolResult(row.message, row.tool.id)">
                    <pre v-if="toolResult(row.message, row.tool.id)?.content" class="message-pre">{{
                      formatToolPayload(toolResult(row.message, row.tool.id)?.content)
                    }}</pre>
                    <div
                      v-if="toolResult(row.message, row.tool.id)?.images?.length"
                      class="image-row history-image-row"
                      aria-label="Tool result images"
                    >
                      <img
                        v-for="(image, index) in toolResult(row.message, row.tool.id)?.images ?? []"
                        :key="index"
                        :src="imageSource(image)"
                        :alt="`Tool image ${index + 1}`"
                        loading="lazy"
                      />
                    </div>
                  </template>
                </details>
                <div v-else-if="row.kind === 'meta'" class="message-meta">
                  <span class="message-time">{{ formatTime(row.message.timestamp) }}</span>
                  <button
                    class="message-copy"
                    type="button"
                    :title="row.message.content ? 'Copy message' : 'Nothing to copy'"
                    :aria-label="row.message.content ? 'Copy message' : 'Nothing to copy'"
                    :disabled="!row.message.content"
                    @click="copyMessage(row.message)"
                  >
                    <Check v-if="copiedKey === row.message.key" :size="13" />
                    <Copy v-else :size="13" />
                  </button>
                </div>
              </div>
            </article>
          </template>
        </div>

        <div v-if="connectionError" class="inline-error">
          <CloudOff :size="16" />
          <span>{{ connectionError }}</span>
          <button class="text-button" @click="openSettings">Configure</button>
        </div>
      </section>

      <footer ref="composerFooter" class="composer" @wheel="handleComposerWheel">
        <form
          class="composer-box"
          @submit.prevent="submitMessage"
          @dragover.prevent
          @drop.prevent="dropImages"
        >
          <input
            ref="imagePicker"
            class="image-picker"
            type="file"
            :accept="IMAGE_ACCEPT"
            multiple
            @change="pickImages"
          />
          <div
            v-if="draftImages.length"
            class="image-row draft-image-row"
            aria-label="Attached images"
          >
            <div v-for="image in draftImages" :key="image.id" class="draft-image">
              <img :src="imageSource(image)" :alt="image.name" :title="image.name" />
              <button
                type="button"
                :disabled="submittingMessage"
                :aria-label="`Remove ${image.name}`"
                @click="draftImages = draftImages.filter((item) => item.id !== image.id)"
              >
                <X :size="14" />
              </button>
            </div>
          </div>
          <div class="composer-input-row">
            <textarea
              ref="draftInput"
              v-model="draft"
              :disabled="!canEditComposer"
              rows="1"
              :placeholder="
                !selectedWorkspace
                  ? 'Select a workspace...'
                  : !stateReady
                    ? 'Waiting for daemon state...'
                    : selectedAgentReady
                      ? selectedAgentWorking
                        ? `${selectedAgentName} is working...`
                        : `Message ${selectedAgentName}...`
                      : 'Agent is not ready...'
              "
              @keydown.enter.exact="onComposerEnter"
            ></textarea>
          </div>
          <div v-if="composerError" class="composer-error" role="alert">{{ composerError }}</div>
          <div class="composer-actions">
            <div ref="uploadControl" class="upload-control">
              <button
                class="icon-button"
                type="button"
                title="Add image"
                aria-label="Add image"
                :aria-expanded="uploadMenuOpen"
                :disabled="!canEditComposer || importingImages"
                @click="uploadMenuOpen = !uploadMenuOpen"
              >
                <Plus :size="19" />
              </button>
              <div v-if="uploadMenuOpen" class="upload-menu">
                <button type="button" @click="openImagePicker">Upload image</button>
              </div>
            </div>
            <span v-if="importingImages" class="image-import-label">Importing images…</span>
            <span v-if="submittingMessage" class="image-import-label" role="status">{{
              sendStage === 'waking'
                ? 'Message injected; waiting for wake acknowledgement…'
                : 'Sending; waiting for daemon acknowledgement…'
            }}</span>
            <span v-if="abortPending" class="abort-pending-label">Abort pending</span>
            <span v-if="selectedModel" class="composer-model" :title="modelTitle">
              <span class="composer-model-name">{{ selectedModel.model }}</span>
              <span class="composer-model-context">{{
                compactTokenNumber(selectedModel.context_window_tokens)
              }}</span>
            </span>
            <button
              v-if="selectedAgentWorking"
              class="send-button send-button--stop"
              type="button"
              :title="
                canAbortTurn
                  ? 'Stop current turn'
                  : abortPending
                    ? 'Abort pending'
                    : 'No abortable turn'
              "
              :disabled="
                !canAbortTurn ||
                !selectedWorkspace ||
                connectionStatus !== 'online' ||
                !stateReady ||
                !selectedAgentReady
              "
              @click="abortTurn"
            >
              <Square :size="14" :stroke-width="0" fill="currentColor" />
            </button>
            <button
              v-else
              class="send-button"
              type="submit"
              title="Send message"
              :disabled="
                (!draft.trim() && !draftImages.length) ||
                importingImages ||
                submittingMessage ||
                !selectedWorkspace ||
                connectionStatus !== 'online' ||
                !stateReady ||
                !selectedAgentIdle
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
            <span class="tool-manager-empty">Resources Panel</span>
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
                <div
                  v-for="choice in sandboxChoices"
                  :key="choice.value || 'none'"
                  class="tool-resource-row"
                  :title="choice.hint"
                >
                  <ShieldCheck v-if="choice.value" :size="14" />
                  <ShieldOff v-else :size="14" />
                  <code :title="choice.hint">{{ choice.label }}</code>
                  <button
                    class="tool-row-action"
                    type="button"
                    :title="
                      sandboxSelection === choice.value ? 'Active sandbox' : 'Use this sandbox'
                    "
                    :aria-label="
                      sandboxSelection === choice.value ? 'Active sandbox' : 'Use this sandbox'
                    "
                    :disabled="
                      connectionStatus !== 'online' ||
                      !stateReady ||
                      !selectedAgentReady ||
                      resourceWriteInFlight
                    "
                    @click="selectSandbox(choice.value)"
                  >
                    <CircleDot v-if="sandboxSelection === choice.value" :size="14" />
                    <Circle v-else :size="14" />
                  </button>
                </div>
              </div>
            </section>

            <section class="tool-resource-group">
              <div class="tool-resource-heading">
                <span>Tools</span>
                <span>{{ toolOptions.length }}</span>
              </div>
              <div v-if="toolOptions.length" class="tool-resource-list">
                <div v-for="alias in toolOptions" :key="alias" class="tool-resource-row">
                  <Wrench :size="14" />
                  <code :title="alias">{{ alias }}</code>
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
        <button
          id="activity-tab"
          type="button"
          role="tab"
          :aria-selected="activityTab === 'activity'"
          aria-controls="activity-tabpanel"
          :class="{ active: activityTab === 'activity' }"
          @click="activityTab = 'activity'"
        >
          Activity
        </button>
        <button
          id="mcl-tab"
          type="button"
          role="tab"
          :aria-selected="activityTab === 'mcl'"
          aria-controls="mcl-tabpanel"
          :class="{ active: activityTab === 'mcl' }"
          @click="activityTab = 'mcl'"
        >
          MCL <span>{{ mclRecords.length }}</span>
        </button>
      </div>
      <div
        v-if="activityTab === 'mcl'"
        id="mcl-tabpanel"
        class="activity-list"
        role="tabpanel"
        aria-labelledby="mcl-tab"
      >
        <p v-if="!mclRecords.length" class="activity-empty">
          No MCL records for the selected agent
        </p>
        <details v-for="record in mclRecords" :key="record.key" class="mcl-audit-entry">
          <summary>
            #{{ record.sequence }} · {{ record.content || 'MCL command' }} ·
            {{ formatTime(record.timestamp) }}
          </summary>
          <pre>{{ metadataText(record.payload) }}</pre>
        </details>
      </div>
      <div
        v-if="activityTab === 'activity'"
        class="activity-filter"
        role="group"
        aria-label="Filter activity"
      >
        <button type="button" :class="{ active: logFilter === 'all' }" @click="logFilter = 'all'">
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
      </div>
      <div
        v-if="activityTab === 'activity'"
        id="activity-tabpanel"
        class="activity-list"
        role="tabpanel"
        aria-labelledby="activity-tab"
      >
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
      <button
        v-if="activityTab === 'activity'"
        class="activity-clear"
        type="button"
        @click="store.clearLogs"
      >
        Clear activity
      </button>
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
        <label class="field-label" for="context-path">Driver context path</label>
        <input
          id="context-path"
          v-model="contextPathDraft"
          class="field-input"
          placeholder="Path defined by your driver"
          @keydown.enter="saveSettings"
        />
        <p class="field-label">
          Driver-defined destination for messages. Change only if your driver uses a different path.
        </p>
        <p v-if="formError" class="form-error">{{ formError }}</p>
        <div class="dialog-actions">
          <button class="secondary-button" @click="settingsOpen = false">Cancel</button>
          <button class="primary-button" @click="saveSettings"><Wifi :size="16" /> Connect</button>
        </div>
      </section>
    </div>
  </div>
</template>
