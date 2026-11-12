<script setup lang="ts">
import { computed, nextTick, onMounted, ref, watch } from 'vue';
import { storeToRefs } from 'pinia';
import { Boxes, ChevronRight, Cpu, History, ShieldCheck, Wrench } from 'lucide-vue-next';
import { useWorkbenchStore } from '@/stores/workbench';
import { isRecord, type HistoryMessage } from '@/types/protocol';
import { formatRecordTime } from '@/lib/time';

const store = useWorkbenchStore();
const {
  selectedAgent,
  selectedAgentState,
  selectedWorkspace,
  selectedHistory,
  historyCursors,
  toolDefinitions,
  taskStates,
  sandboxOptions,
} = storeToRefs(store);

const historyList = ref<HTMLElement | null>(null);
const olderPending = ref(false);
let olderAnchor: { height: number; top: number } | null = null;
let olderTimer: ReturnType<typeof setTimeout> | undefined;

const address = computed(() => selectedAgent.value ?? '');
const agent = computed(() => selectedAgentState.value);
const model = computed(() => agent.value?.model ?? null);
const tools = computed(() => toolDefinitions.value.get(address.value) ?? []);
const tasks = computed(() => taskStates.value.get(address.value) ?? []);
const history = computed<HistoryMessage[]>(() => selectedHistory.value?.messages ?? []);
const hasOlder = computed(() => {
  const cursor = historyCursors.value.get(address.value);
  return cursor === undefined || cursor.before !== null;
});
const sandboxActive = computed(() => agent.value?.sandbox ?? null);

function payloadOf(message: HistoryMessage): Record<string, unknown> {
  return isRecord(message.payload) ? message.payload : {};
}

function messageType(message: HistoryMessage): string {
  const type = payloadOf(message).type;
  return typeof type === 'string' ? type : 'message';
}

function historyPreview(message: HistoryMessage): string {
  const payload = payloadOf(message);
  if (Array.isArray(payload.tool_calls) && payload.tool_calls.length) {
    const names = payload.tool_calls
      .map((call) => (isRecord(call) && typeof call.tool_name === 'string' ? call.tool_name : 'tool'))
      .join(', ');
    return `tool calls: ${names}`;
  }
  if (typeof payload.tool_name === 'string')
    return `${payload.tool_name} ${typeof payload.arguments === 'string' ? payload.arguments : ''}`.trim();
  if (Array.isArray(payload.messages)) return `inject ${payload.messages.length} message(s)`;
  if (typeof payload.content === 'string' && payload.content.trim())
    return payload.content.replace(/\s+/g, ' ');
  if (typeof payload.reasoning === 'string' && payload.reasoning.trim())
    return payload.reasoning.replace(/\s+/g, ' ');
  if (typeof payload.message === 'string') return payload.message;
  return message.content || '(empty)';
}

function historyDetail(message: HistoryMessage): string {
  return JSON.stringify(message.payload, null, 2) ?? String(message.content);
}

function outputText(output: unknown[]): string {
  return output
    .map((value) => (typeof value === 'string' ? value : JSON.stringify(value, null, 2)))
    .join('\n');
}

function scrollHistoryBottom() {
  const list = historyList.value;
  if (list) list.scrollTop = list.scrollHeight;
}

function loadOlder() {
  const list = historyList.value;
  if (!list || olderPending.value || !hasOlder.value) return;
  olderAnchor = { height: list.scrollHeight, top: list.scrollTop };
  olderPending.value = true;
  store.loadOlderHistory();
  clearTimeout(olderTimer);
  olderTimer = setTimeout(() => {
    olderPending.value = false;
    olderAnchor = null;
  }, 1500);
}

function onHistoryScroll() {
  const list = historyList.value;
  if (!list || olderPending.value || list.scrollTop > 64) return;
  loadOlder();
}

watch(
  () => history.value.length,
  async (next, previous) => {
    if (!olderAnchor || next <= previous) return;
    await nextTick();
    const list = historyList.value;
    if (list) list.scrollTop = list.scrollHeight - olderAnchor.height + olderAnchor.top;
    olderPending.value = false;
    olderAnchor = null;
    clearTimeout(olderTimer);
  },
);

watch(
  () => history.value.length,
  async (next, previous) => {
    if (olderAnchor || next <= previous) return;
    await nextTick();
    scrollHistoryBottom();
  },
);

watch(selectedAgent, async () => {
  await nextTick();
  scrollHistoryBottom();
});

onMounted(async () => {
  await nextTick();
  scrollHistoryBottom();
});
</script>

<template>
  <div class="info-panel">
    <div v-if="!selectedAgent" class="shell-empty">
      <div class="empty-icon"><Boxes :size="22" /></div>
      <strong>No agent selected</strong>
      <span>Select an Agent to inspect its resources.</span>
    </div>
    <div v-else class="info-scroll">
      <section class="info-hero">
        <div>
          <span class="eyebrow">Agent</span>
          <h2>{{ agent?.agent.split('/').pop()?.split(':')[0] ?? selectedAgent }}</h2>
          <code>{{ selectedAgent }}</code>
        </div>
        <span class="info-status">{{ agent?.status ?? 'unknown' }}</span>
      </section>

      <p v-if="agent?.error" class="info-error info-agent-error">
        {{ agent.error.code }}: {{ agent.error.message }}
      </p>

      <section class="info-grid">
        <div><span>Workspace</span><code>{{ selectedWorkspace?.name ?? agent?.workspace.id ?? '—' }}</code></div>
        <div><span>Turn</span><code>{{ agent?.turn ?? 'idle' }}</code></div>
        <div><span>Sandbox</span><code>{{ sandboxActive ?? 'none' }}</code></div>
        <div><span>Context</span><code>{{ model ? `${model.context_window_tokens.toLocaleString()} tokens` : '—' }}</code></div>
      </section>

      <section class="info-section">
        <h3><Cpu :size="14" /> Model</h3>
        <div v-if="model" class="info-model">
          <div><span>Model</span><code>{{ model.model }}</code></div>
          <div><span>Route</span><code>{{ model.id }}</code></div>
          <div><span>Provider / API</span><code>{{ model.provider }} · {{ model.api_type }}</code></div>
          <div><span>Thinking / effort</span><code>{{ model.thinking ?? '—' }} · {{ model.reasoning_effort ?? '—' }}</code></div>
        </div>
        <p v-else class="info-empty">Model information is not available.</p>
      </section>

      <section class="info-section">
        <h3><ShieldCheck :size="14" /> Sandboxes <span>{{ sandboxOptions.options.length }}</span></h3>
        <div v-if="sandboxOptions.options.length" class="info-list">
          <div v-for="alias in sandboxOptions.options" :key="alias" class="info-row">
            <ShieldCheck :size="13" />
            <code>{{ alias }}</code>
            <span v-if="alias === sandboxActive" class="info-badge">active</span>
          </div>
        </div>
        <p v-else class="info-empty">No sandboxes imported.</p>
      </section>

      <section class="info-section">
        <h3><Wrench :size="14" /> Tools <span>{{ tools.length }}</span></h3>
        <div v-if="tools.length" class="info-list">
          <details v-for="tool in tools" :key="tool.alias" class="info-tool">
            <summary>
              <code>{{ tool.alias }}</code>
              <span>{{ tool.description }}</span>
            </summary>
            <p class="info-description">{{ tool.description }}</p>
            <p class="info-path">{{ tool.path }}</p>
            <p v-if="tool.owner_task_id" class="info-path">owner: {{ tool.owner_task_id }}</p>
            <pre v-if="tool.input_schema">{{ JSON.stringify(tool.input_schema, null, 2) }}</pre>
            <pre v-if="tool.binding !== null">{{ JSON.stringify(tool.binding, null, 2) }}</pre>
          </details>
        </div>
        <p v-else class="info-empty">No tools imported.</p>
      </section>

      <section class="info-section">
        <h3><Boxes :size="14" /> Tasks <span>{{ tasks.length }}</span></h3>
        <div v-if="tasks.length" class="info-list">
          <details v-for="task in tasks" :key="task.task_id" class="info-task">
            <summary>
              <code>{{ task.alias }}</code>
              <span class="info-badge">{{ task.returned ? 'returned' : 'running' }}</span>
              <small>{{ task.task_id }}</small>
            </summary>
            <p class="info-path">call: {{ task.call_id }}</p>
            <p v-if="task.error" class="info-error">{{ task.error.code }}: {{ task.error.message }}</p>
            <pre v-if="task.output.length">{{ outputText(task.output) }}</pre>
          </details>
        </div>
        <p v-else class="info-empty">No active tasks.</p>
      </section>

      <section class="info-section">
        <h3>
          <History :size="14" /> History <span>{{ history.length }}</span>
          <span v-if="olderPending" class="info-loading">loading…</span>
        </h3>
        <div
          ref="historyList"
          class="info-history"
          aria-label="Agent history"
          @scroll="onHistoryScroll"
        >
          <details
            v-for="message in history"
            :key="message.sequence"
            class="info-history-entry"
          >
            <summary class="info-history-row">
              <span class="info-history-time">{{ formatRecordTime(message.created_at_ms, true) }}</span>
              <span class="info-badge">{{ messageType(message) }}</span>
              <span class="info-history-seq">#{{ message.sequence }}</span>
              <span class="info-history-text">{{ historyPreview(message) }}</span>
              <ChevronRight class="info-history-caret" :size="12" />
            </summary>
            <pre class="info-history-detail">{{ historyDetail(message) }}</pre>
          </details>
          <p v-if="!history.length" class="info-empty">No persisted history.</p>
        </div>
      </section>
    </div>
  </div>
</template>
