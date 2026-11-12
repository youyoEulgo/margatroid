import { computed, ref } from 'vue';
import { defineStore } from 'pinia';
import {
  parseServerMessage,
  isRecord,
  isAgentMessage,
  type AgentHistory,
  type AgentMessage,
  type AgentState,
  type ClientInfo,
  type ClientMessage,
  type HistoryMessage,
  type ServerMessage,
  type StreamDelta,
  type ToolCall,
  type WorkspaceConfig,
  type WorkspaceInfo,
  type WorkspaceReference,
  type UserImage,
  type WorkspaceSnapshot,
  type AgentInfo,
  type TaskSnapshot,
  type AgentTool,
  type AgentLogRecord,
  type AgentMclRecord,
  type RangeQuery,
  type MclData,
} from '@/types/protocol';
import { randomId } from '@/lib/id';

export type ConnectionStatus = 'offline' | 'connecting' | 'online';
const COMMAND_RESULT: Partial<Record<ClientMessage['type'], ServerMessage['type']>> = {
  'workspace.start': 'workspace.start.result',
  'workspace.stop': 'workspace.stop.result',
  'workspace.restart': 'workspace.restart.result',
  'workspace.reload': 'workspace.reload.result',
  'agent.start': 'agent.start.result',
  'agent.stop': 'agent.stop.result',
  'agent.restart': 'agent.restart.result',
  'agent.turn.abort': 'agent.turn.abort.result',
  'mcl.execute': 'mcl.execute.result',
};
export type MessageRole = 'system' | 'user' | 'assistant' | 'tool' | 'error' | 'record';
export interface ToolResult {
  content: string;
  failed: boolean;
  images?: UserImage[];
}
export interface ConversationEntry {
  key: string;
  id: string;
  workspaceKey: string;
  agent: string;
  role: MessageRole;
  thinking: string;
  content: string;
  toolCalls: ToolCall[];
  failed: boolean;
  images?: UserImage[];
  timestamp: number;
  kind: string;
  payload: unknown;
  source: string | null;
  messageType: string | null;
  sequence: number | null;
  toolCallId: string | null;
  toolName: string | null;
  toolResults?: Map<string, ToolResult>;
}
export interface RuntimeLog {
  key: string;
  timestamp: number;
  level: string;
  target: string;
  message: string;
  fields: { name: string; value: string }[];
}
interface StreamEntry extends ConversationEntry {

  lastSequence: number;
  completed: boolean;
  frozen: boolean;
}
interface Target {
  workspace: string;
  agent: string;

}
interface PendingCommand {
  resolve: (value: unknown) => void;
  reject: (error: Error) => void;
  timeout: ReturnType<typeof setTimeout>;
  target?: Target;
  read?: boolean;
  turn?: string;
  expectedType?: ServerMessage['type'];
}
interface AbortGuard extends Target {
  id: string;
  turn: string;
  failed: boolean;
  streamKeys: Set<string>;
}
export type TreeStatus = 'stopped' | 'running' | 'error';
export interface SidebarAgent {
  key: string;
  id: string | null;
  name: string;
  driver: string;
  status: TreeStatus;
  reason: string;
  active: boolean;
}
export interface SidebarWorkspace {
  key: string;
  name: string;
  path: string;
  address: string | null;
  projectRoot: string;
  status: TreeStatus;
  lifecycle: string;
  reason: string;
  active: boolean;
  expanded: boolean;
  agents: SidebarAgent[];
}
export interface SidebarModel {
  workspaces: SidebarWorkspace[];
  selectedKey: string;
}
const MAX_LOGS = 500;
export function workspaceKey(workspace: WorkspaceReference): string {
  return typeof workspace === 'string' ? workspace : workspace.id;
}
export function resourceName(resource: string): string {
  return resource.split('/').pop()?.split(':')[0] || resource;
}
export function contextInjection(
  content: string,
  path: string,
  images: UserImage[] = [],
): { command: string; arguments: unknown[] } {
  if (
    !/^[A-Za-z_][A-Za-z0-9_-]*\.[A-Za-z_][A-Za-z0-9_-]*(?:\[-?\d+(?:\s*,\s*-?\d+)?\])?$/.test(
      path.trim(),
    )
  )
    throw new Error('Configure a valid driver block.field selector before injecting a message');
  return {
    command: `INJECT ? TO ${path.trim()}`,
    arguments: [
      [
        {
          type: 'user',
          content,
          ...(images.length
            ? { data: images.map(({ media_type, data }) => ({ media_type, data })) }
            : {}),
        },
      ],
    ],
  };
}
export function parseArguments(raw: string): unknown[] {
  const value: unknown = JSON.parse(raw);
  if (!Array.isArray(value))
    throw new Error('Arguments must be a JSON array, with one value per ? placeholder');
  return value;
}
function sameImages(left: UserImage[], right: UserImage[]): boolean {
  return (
    left.length === right.length &&
    left.every(
      (image, index) =>
        image.media_type === right[index]!.media_type && image.data === right[index]!.data,
    )
  );
}
export function historyMessage(entry: HistoryMessage): AgentMessage | null {
  const payload =
    isRecord(entry.payload) && isRecord(entry.payload.message)
      ? entry.payload.message
      : entry.payload;
  if (isAgentMessage(payload)) return payload;
  if (entry.kind !== 'message' || !isRecord(payload)) return null;
  let candidate: unknown;
  if (payload.type === 'error') candidate = { ...payload, message: entry.content };
  else if (payload.type === 'assistant')
    candidate = {
      ...payload,
      content: payload.content ?? entry.content,
      reasoning: payload.reasoning ?? null,
      tool_calls: payload.tool_calls ?? [],
    };
  else candidate = { ...payload, content: entry.content };
  return isAgentMessage(candidate) ? candidate : null;
}
function payloadRequestId(entry: HistoryMessage): string {
  const payload =
    isRecord(entry.payload) && isRecord(entry.payload.message)
      ? entry.payload.message
      : entry.payload;
  if (isRecord(payload)) {
    if (typeof payload.request_id === 'string') return payload.request_id;
    if (typeof payload.id === 'string') return payload.id;
  }
  return '';
}
function decodeMessage(
  message: AgentMessage,
): Pick<
  ConversationEntry,
  'role' | 'thinking' | 'content' | 'toolCalls' | 'failed' | 'images' | 'toolCallId' | 'toolName'
>[] {
  switch (message.type) {
    case 'inject':
      return message.messages.flatMap(decodeMessage);
    case 'system':
    case 'user':
      return [
        {
          role: message.type,
          thinking: '',
          content: message.content,
          toolCalls: [],
          failed: false,
          images: message.type === 'user' ? message.data : undefined,
          toolCallId: null,
          toolName: null,
        },
      ];
    case 'assistant':
      return [
        {
          role: 'assistant',
          thinking: message.reasoning ?? '',
          content: message.content ?? '',
          toolCalls: message.tool_calls ?? [],
          failed: false,
          toolCallId: null,
          toolName: null,
        },
      ];
    case 'tool_call':
      return [
        {
          role: 'tool',
          thinking: '',
          content: message.content,
          toolCalls: [],
          failed: message.failed,
          images: message.data,
          toolCallId: message.tool_call_id,
          toolName: message.tool_name,
        },
      ];
    case 'error':
      return [
        {
          role: 'error',
          thinking: '',
          content: message.message,
          toolCalls: [],
          failed: true,
          toolCallId: null,
          toolName: null,
        },
      ];
  }
}
export function historyEntries(history: AgentHistory): ConversationEntry[] {
  const entries: ConversationEntry[] = [...history.messages]
    .sort((a, b) => a.sequence - b.sequence)
    .flatMap((entry) => {
      const base = {
        key: `${workspaceKey(history.workspace)}:${history.agent}:${entry.sequence}`,
        id: payloadRequestId(entry),
        workspaceKey: workspaceKey(history.workspace),
        agent: history.agent,
        timestamp: entry.created_at_ms,
        source: entry.kind === 'mcl' ? (entry.source ?? null) : null,
        messageType: entry.type ?? null,
        sequence: entry.sequence,
        kind: entry.kind,
        payload: entry.payload,
      };
      const message = historyMessage(entry);
      return message
        ? decodeMessage(message).map((value, index) => ({
            ...base,
            ...value,
            key: `${base.key}:${index}`,
          }))
        : [
            {
              ...base,
              role: 'record' as const,
              thinking: '',
              content: entry.content,
              toolCalls: [],
              failed: false,
              toolCallId: null,
              toolName: null,
            },
          ];
    });
  const results = new Map<string, ToolResult>();
  for (const entry of entries)
    if (entry.role === 'tool' && entry.toolCallId)
      results.set(entry.toolCallId, {
        content: entry.content,
        failed: entry.failed,
        images: entry.images,
      });
  const callIds = new Set<string>();
  for (const entry of entries) {
    if (entry.role !== 'assistant' || !entry.toolCalls.length) continue;
    const attached = new Map<string, ToolResult>();
    for (const call of entry.toolCalls) {
      callIds.add(call.id);
      const result = results.get(call.id);
      if (result) attached.set(call.id, result);
    }
    if (attached.size) entry.toolResults = attached;
  }
  // A tool result belongs to its call row; only unpaired results stay standalone.
  return entries.filter(
    (entry) => !(entry.role === 'tool' && entry.toolCallId && callIds.has(entry.toolCallId)),
  );
}
export function latestContextTokens(history: AgentHistory | undefined): number | null {
  let latest: number | null = null;
  function visit(message: AgentMessage) {
    if (message.type === 'inject') message.messages.forEach(visit);
    else if (message.type === 'assistant') {
      const usage = message.usage;
      latest =
        usage &&
        Number.isFinite(usage.input_tokens) &&
        usage.input_tokens >= 0 &&
        Number.isFinite(usage.output_tokens) &&
        usage.output_tokens >= 0
          ? usage.input_tokens + usage.output_tokens
          : null;
    }
  }
  for (const row of [...(history?.messages ?? [])].sort((a, b) => a.sequence - b.sequence)) {
    if (row.kind !== 'message') continue;
    const message = historyMessage(row);
    if (message) visit(message);
  }
  return latest;
}
export function buildSidebarModel(
  definitions: WorkspaceConfig[],
  runtime: WorkspaceInfo[],
  states: AgentState[],
  failures: Map<string, { message: string }>,
  expanded: Set<string>,
  selectedKey: string,
  selectedAgent: string | null,
): SidebarModel {
  return {
    selectedKey,
    workspaces: definitions.map((definition) => {
      const instance = runtime.find((candidate) => candidate.definition_path === definition.path);
      const ownedStates = instance
        ? states.filter((state) => workspaceKey(state.workspace) === instance.id)
        : [];
      const advertised = [
        ...new Set([...(instance?.agents ?? []), ...ownedStates.map((state) => state.agent)]),
      ];
      const rows = definition.cache.agents.map((member) => ({
        name: member.name,
        driver: member.driver,
        id: advertised.find((id) => resourceName(id) === member.name) ?? null,
      }));
      for (const id of advertised)
        if (!rows.some((row) => row.id === id))
          rows.push({ id, name: resourceName(id), driver: '' });
      const agents: SidebarAgent[] = rows.map((member) => {
        const state = ownedStates.find((candidate) => candidate.agent === member.id);
        const memberFailure = instance?.member_failures?.find(
          (failure) =>
            failure.agent === member.id ||
            failure.agent === member.name ||
            resourceName(failure.agent) === member.name,
        )?.error;
        const reason =
          memberFailure ||
          state?.error?.message ||
          (instance && member.id ? failures.get(`${instance.id}:${member.id}`)?.message : '') ||
          '';
        return {
          ...member,
          key: member.id ?? member.name,
          reason,
          status: reason
            ? 'error'
            : state && ['idle', 'working'].includes(state.status)
              ? 'running'
              : 'stopped',
          active:
            definition.path === selectedKey && member.id !== null && member.id === selectedAgent,
        };
      });
      const reason =
        agents.find((agent) => agent.reason)?.reason ||
        instance?.error?.message ||
        instance?.member_failures?.[0]?.error ||
        '';
      const status: TreeStatus = reason
        ? 'error'
        : instance?.status === 'running' && agents.every((agent) => agent.status === 'running')
          ? 'running'
          : 'stopped';
      return {
        key: definition.path,
        path: definition.path,
        address: instance?.id ?? null,
        name: definition.cache.name,
        projectRoot: definition.cache.project_root,
        status,
        lifecycle: instance?.status ?? 'stopped',
        reason,
        agents,
        active: definition.path === selectedKey,
        expanded: expanded.has(definition.path),
      };
    }),
  };
}
function defaultEndpoint(): string {
  return typeof window === 'undefined'
    ? 'ws://127.0.0.1:3939/ws'
    : `${window.location.protocol === 'https:' ? 'wss:' : 'ws:'}//${window.location.host}/ws`;
}
function normalizeEndpoint(raw: string): string {
  let value = raw.trim();
  if (!value) throw new Error('Daemon URL is required');
  if (!value.includes('://')) value = `ws://${value}`;
  const url = new URL(value);
  if (url.protocol === 'http:') url.protocol = 'ws:';
  if (url.protocol === 'https:') url.protocol = 'wss:';
  if (!['ws:', 'wss:'].includes(url.protocol))
    throw new Error('Daemon URL must use ws:// or wss://');
  if (url.pathname === '/') url.pathname = '/ws';
  return url.toString();
}

export const useWorkbenchStore = defineStore('workbench', () => {
  const endpoint = ref(defaultEndpoint());
  const driverContextPath = ref('entry.pending[-1]');
  const connectionStatus = ref<ConnectionStatus>('offline');
  const stateReady = ref(false);
  const connectionError = ref('');
  const registeredClient = ref<ClientInfo | null>(null);
  const workspaceSnapshots = ref<WorkspaceSnapshot[]>([]);
  const activeAgentInfos = ref<AgentInfo[]>([]);
  const turnStates = ref(new Map<string, string | null>());
  const taskStates = ref(new Map<string, TaskSnapshot[]>());
  const toolDefinitions = ref(new Map<string, AgentTool[]>());
  const agentLogs = ref(new Map<string, AgentLogRecord[]>());
  const mclRecords = ref(new Map<string, AgentMclRecord[]>());
  const historyCursors = ref(new Map<string, { before: number | null; after: number | null }>());
  const recordQueries = new Map<string, Target>();
  const historyQueries = new Map<string, Target>();
  const definitions = ref<WorkspaceConfig[]>([]);
  const runtimeWorkspaces = ref<WorkspaceInfo[]>([]);
  const agentStates = ref<AgentState[]>([]);
  const histories = ref<AgentHistory[]>([]);
  const selectedWorkspaceKey = ref('');
  const selectedAgent = ref<string | null>(null);
  const expandedWorkspaceKeys = ref(new Set<string>());
  const workspaceOperationPending = ref(false);
  const workspaceNotice = ref('');
  const logs = ref<RuntimeLog[]>([]);
  const lastFailures = ref(new Map<string, { message: string }>());
  const streams = ref(new Map<string, StreamEntry>());
  const abortGuards = ref(new Map<string, AbortGuard>());
  const pending = new Map<string, PendingCommand>();
  const completedStreams = new Set<string>();
  const abortedStreams = new Set<string>();
  const sendingTargets = new Set<string>();
  const pendingWakes = new Map<
    string,
    { workspace: string; agent: string; content: string; images: UserImage[] }
  >();
  const resourceWritePending = ref('');
  const resourceWriteInFlight = computed(() => !!resourceWritePending.value);
  // Tools are advertised by the Agent state and are read-only in this panel.
  const toolOptions = computed(() => selectedAgentState.value?.tools ?? []);
  const sandboxOptions = computed(() => ({
    options: selectedAgentState.value?.sandboxes ?? [],
    activated: selectedAgentState.value?.sandbox ? [selectedAgentState.value.sandbox] : [],
  }));
  let socket: WebSocket | null = null;
  let registrationId = '';
  const listQueries = new Map<string, 'workspace.req' | 'agent.req'>();
  let routeRevision = 0;
  let initializedSelection = false;
  let autoSelectSingle = true;
  let reconnectTimer: ReturnType<typeof setTimeout> | undefined;
  let reconnectAttempt = 0;
  let shouldReconnect = true;
  const workspaces = computed(() =>
    definitions.value.map((definition) => {
      const runtime = runtimeWorkspaces.value.find(
        (candidate) => candidate.definition_path === definition.path,
      );
      return {
        path: definition.path,
        id: runtime?.id ?? '',
        name: definition.cache.name,
        project_root: runtime?.project_root ?? definition.cache.project_root,
        agents: runtime?.agents ?? [],
      };
    }),
  );
  const selectedDefinition = computed(
    () =>
      definitions.value.find((definition) => definition.path === selectedWorkspaceKey.value) ??
      null,
  );
  const selectedWorkspace = computed(
    () =>
      workspaces.value.find((workspace) => workspace.path === selectedWorkspaceKey.value) ?? null,
  );
  const selectedRuntimeWorkspace = computed(
    () =>
      runtimeWorkspaces.value.find(
        (workspace) => workspace.definition_path === selectedWorkspaceKey.value,
      ) ?? null,
  );
  const selectedAgentState = computed(
    () =>
      agentStates.value.find(
        (state) =>
          state.agent === selectedAgent.value &&
          workspaceKey(state.workspace) === selectedRuntimeWorkspace.value?.id,
      ) ?? null,
  );
  const selectedAgentName = computed(() =>
    selectedAgent.value ? resourceName(selectedAgent.value) : 'Choose an agent',
  );
  const selectedAgentReady = computed(
    () =>
      ['idle', 'working'].includes(selectedAgentState.value?.status ?? ''),
  );
  const selectedAgentIdle = computed(
    () => selectedAgentReady.value && selectedAgentState.value?.status === 'idle',
  );
  const selectedAgentWorking = computed(() => selectedAgentState.value?.status === 'working');
  const selectedAbortGuard = computed(() =>
    [...abortGuards.value.values()].find(
      (guard) =>
        guard.workspace === selectedRuntimeWorkspace.value?.id &&
        guard.agent === selectedAgent.value,
    ),
  );
  const abortPending = computed(
    () =>
      selectedAgentWorking.value && (!selectedAgentState.value?.turn || !!selectedAbortGuard.value),
  );
  const canAbortTurn = computed(
    () => selectedAgentWorking.value && !!selectedAgentState.value?.turn && !abortPending.value,
  );
  const sidebarTree = computed(() =>
    buildSidebarModel(
      definitions.value,
      runtimeWorkspaces.value,
      agentStates.value,
      lastFailures.value,
      expandedWorkspaceKeys.value,
      selectedWorkspaceKey.value,
      selectedAgent.value,
    ),
  );
  const selectedHistory = computed(() =>
    histories.value.find(
      (history) =>
        workspaceKey(history.workspace) === selectedRuntimeWorkspace.value?.id &&
        history.agent === selectedAgent.value,
    ),
  );
  const selectedHistoryEntries = computed(() =>
    selectedHistory.value ? historyEntries(selectedHistory.value) : [],
  );
  const historyMessageIds = computed(
    () =>
      new Set(
        selectedHistoryEntries.value
          .filter((entry) => entry.role === 'assistant')
          .map((entry) => entry.id),
      ),
  );
  const visibleMessages = computed(() => {
    const history = selectedHistoryEntries.value;
    const live = [...streams.value.values()].filter(
      (entry) =>
        entry.workspaceKey === selectedRuntimeWorkspace.value?.id &&
        entry.agent === selectedAgent.value &&
        !historyMessageIds.value.has(entry.id),
    );
    return [...history, ...live];
  });
  const selectedMclEntries = computed(() => {
    const target = currentTarget(false);
    if (!target) return [];
    const workspace = selectedAgentState.value!.workspace;
    return historyEntries({
      workspace,
      agent: target.agent,
      messages: (mclRecords.value.get(targetKey(target)) ?? []).map((r) => ({ ...r, kind: 'mcl' })),
    });
  });
  const selectedSessionLogs = computed<RuntimeLog[]>(() => {
    const target = currentTarget(false);
    if (!target) return [];
    return (agentLogs.value.get(targetKey(target)) ?? []).map((r) => ({
      key: `${target.agent}:log:${r.sequence}`,
      timestamp: r.created_at_ms,
      level: r.level.toUpperCase(),
      target: r.source,
      message: r.message,
      fields: [{ name: 'stage', value: r.stage }],
    }));
  });
  const contextTokens = computed(() => latestContextTokens(selectedHistory.value));
  const contextWindowTokens = computed(
    () => selectedAgentState.value?.context_window_tokens ?? null,
  );
  const tokenUsage = computed(() => {
    let input = 0,
      output = 0,
      cached = 0;
    for (const row of selectedHistory.value?.messages ?? []) {
      const value =
        isRecord(row.payload) && isRecord(row.payload.message) ? row.payload.message : row.payload;
      const usage = isRecord(value) && isRecord(value.usage) ? value.usage : null;
      if (!usage) continue;
      input += typeof usage.input_tokens === 'number' ? usage.input_tokens : 0;
      output += typeof usage.output_tokens === 'number' ? usage.output_tokens : 0;
      cached += typeof usage.cache_hit_tokens === 'number' ? usage.cache_hit_tokens : 0;
    }
    return { input, output, cached };
  });
  function addLog(
    level: string,
    message: string,
    target = 'ui',
    fields: RuntimeLog['fields'] = [],
  ) {
    logs.value.push({
      key: randomId(),
      timestamp: Date.now(),
      level,
      message,
      target,
      fields,
    });
    if (logs.value.length > MAX_LOGS) logs.value.splice(0, logs.value.length - MAX_LOGS);
  }
  function connected(): boolean {
    return socket?.readyState === WebSocket.OPEN && registeredClient.value !== null;
  }
  function sendQuery(type: 'workspace.req' | 'agent.req') {
    if (connected()) {
      const id = randomId();
      listQueries.set(id, type);
      socket!.send(JSON.stringify({ type, id, message: {} } satisfies ClientMessage));
    }
  }
  function queryState() {
    sendQuery('workspace.req');
    sendQuery('agent.req');
    const target = currentTarget(false);
    if (target) queryAgentDetails(target);
  }
  function targetKey(target: Pick<Target, 'agent'>): string {
    return `${target.agent}`;
  }
  function matchesTarget(target: Pick<Target, 'agent'>): boolean {
    return agentStates.value.some(
      (s) => s.agent === target.agent,
    );
  }
  function queryAgentDetails(target: Target) {
    if (!connected()) return;
    for (const type of ['agent.turn.req', 'agent.task.req'] as const) {
      const id = randomId();
      recordQueries.set(id, target);
      socket!.send(
        JSON.stringify({
          type,
          id,
          message: { address: target.agent },
        } satisfies ClientMessage),
      );
    }
  }
  function queryHistoryFor(target: Target, range: RangeQuery = {}) {
    if (!connected() || !matchesTarget(target)) return;
    const id = randomId();
    recordQueries.set(id, target);
    historyQueries.set(id,target);
    socket!.send(
      JSON.stringify({
        type: 'agent.history.req',
        id,
        message: { address: target.agent, limit: 50, ...range },
      } satisfies ClientMessage),
    );
  }
  function queryRecords(kind: 'log' | 'mcl', range: RangeQuery = {}) {
    const target = currentTarget(false);
    if (!target || !connected()) return;
    const id = randomId();
    recordQueries.set(id, target);
    socket!.send(
      JSON.stringify({
        type: kind === 'log' ? 'agent.log.req' : 'agent.mcl.req',
        id,
        message: { address: target.agent, limit: 50, ...range },
      } satisfies ClientMessage),
    );
  }
  function queryHistory() {
    const target = currentTarget(false);
    if (target) {
      queryAgentDetails(target);
      queryHistoryFor(target);
    }
  }
  function loadOlderHistory() {
    const target = currentTarget(false);
    if (!target) return;
    const sequence = selectedHistory.value?.messages.reduce<number | null>(
      (oldest, row) => (oldest === null ? row.sequence : Math.min(oldest, row.sequence)),
      null,
    );
    if (
      sequence != null &&
      ![...historyQueries.values()].some(
        (q) => q.agent === target.agent,
      )
    )
      queryHistoryFor(target, { before_sequence: sequence });
  }
  function rejectPending(message: string) {
    for (const command of pending.values()) {
      clearTimeout(command.timeout);
      command.reject(new Error(message));
    }
    pending.clear();
  }
  function resourcesReset() {
    resourceWritePending.value = '';
  }
  function closeSocket() {
    if (socket) {
      socket.onclose = null;
      socket.close();
      socket = null;
    }
    registeredClient.value = null;
    stateReady.value = false;
    resourcesReset();
    recordQueries.clear();
    historyQueries.clear();
    listQueries.clear();
    registrationId = '';
    pendingWakes.clear();
    streams.value = new Map();
    rejectPending('Backend WebSocket closed');
  }
  function connect(rawEndpoint?: string): boolean {
    if (rawEndpoint !== undefined) {
      try {
        endpoint.value = normalizeEndpoint(rawEndpoint);
      } catch (error) {
        connectionError.value = error instanceof Error ? error.message : String(error);
        return false;
      }
    }
    shouldReconnect = true;
    clearTimeout(reconnectTimer);
    closeSocket();
    connectionError.value = '';
    connectionStatus.value = 'connecting';
    const next = new WebSocket(endpoint.value);
    socket = next;
    next.onopen = () => {
      if (socket !== next) return;
      registrationId = randomId();
      next.send(
        JSON.stringify({
          type: 'connection.register',
          id: registrationId,
          message: { client_type: 'webui', name: `webui-${randomId()}` },
        } satisfies ClientMessage),
      );
    };
    next.onmessage = (event) => {
      if (socket !== next || typeof event.data !== 'string') return;
      const parsed = parseServerMessage(event.data);
      if (parsed) handleServerMessage(parsed);
      else addLog('WARN', 'Ignored an invalid or unsupported backend message');
    };
    next.onerror = () => {
      if (socket === next) connectionError.value = 'Unable to reach the daemon';
    };
    next.onclose = () => {
      if (socket !== next) return;
      socket = null;
      registeredClient.value = null;
      stateReady.value = false;
      connectionStatus.value = 'offline';
      streams.value = new Map();
      pendingWakes.clear();
      resourcesReset();
      rejectPending('Backend WebSocket disconnected');
      if (shouldReconnect)
        reconnectTimer = setTimeout(
          () => connect(),
          Math.min(1000 * 2 ** reconnectAttempt++, 10000),
        );
    };
    return true;
  }
  function disconnect() {
    shouldReconnect = false;
    clearTimeout(reconnectTimer);
    closeSocket();
    connectionStatus.value = 'offline';
  }
  function selectWorkspace(path: string) {
    initializedSelection = true;
    if (path === selectedWorkspaceKey.value) return;
    routeRevision++;
    autoSelectSingle = true;
    selectedWorkspaceKey.value = definitions.value.some((definition) => definition.path === path)
      ? path
      : '';
    const agents = selectedRuntimeWorkspace.value?.agents ?? [];
    selectedAgent.value = agents.length === 1 ? agents[0]! : null;
    if (selectedWorkspaceKey.value) expandWorkspace(selectedWorkspaceKey.value);
    queryHistory();
  }
  function selectAgent(agent: string | null, path = selectedWorkspaceKey.value) {
    if (path !== selectedWorkspaceKey.value) selectWorkspace(path);
    const accepted = agent && selectedRuntimeWorkspace.value?.agents.includes(agent) ? agent : null;
    if (accepted !== selectedAgent.value) routeRevision++;
    selectedAgent.value = accepted;
    autoSelectSingle = accepted !== null;
    queryHistory();
  }
  function expandWorkspace(path: string) {
    expandedWorkspaceKeys.value = new Set(expandedWorkspaceKeys.value).add(path);
  }
  function toggleWorkspace(path: string) {
    const expanded = expandedWorkspaceKeys.value.has(path);
    if (path !== selectedWorkspaceKey.value) selectWorkspace(path);
    const next = new Set(expandedWorkspaceKeys.value);
    if (expanded) next.delete(path);
    else next.add(path);
    expandedWorkspaceKeys.value = next;
  }
  function reconcileSelection() {
    const paths = new Set(definitions.value.map((definition) => definition.path));
    expandedWorkspaceKeys.value = new Set(
      [...expandedWorkspaceKeys.value].filter((path) => paths.has(path)),
    );
    if (!initializedSelection && definitions.value.length)
      selectWorkspace(definitions.value[0]!.path);
    else if (selectedWorkspaceKey.value && !paths.has(selectedWorkspaceKey.value))
      selectWorkspace('');
    const members = selectedRuntimeWorkspace.value?.agents ?? [];
    if (selectedAgent.value && !members.includes(selectedAgent.value)) {
      routeRevision++;
      selectedAgent.value = null;
    }
    if (autoSelectSingle && !selectedAgent.value && members.length === 1) {
      routeRevision++;
      selectedAgent.value = members[0]!;
      queryHistory();
    }
  }
  function currentTarget(required = true): Target | null {
    const workspace = selectedRuntimeWorkspace.value;
    const state = selectedAgentState.value;
    if (!workspace || !selectedAgent.value || !state || !selectedAgentReady.value) {
      if (required) throw new Error('Select a running agent explicitly');
      return null;
    }
    return { workspace: workspace.id, agent: selectedAgent.value };
  }
  function request(request: ClientMessage, target?: Target, read = false): Promise<unknown> {
    if (!connected()) return Promise.reject(new Error('Register a daemon connection first'));
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        pending.delete(request.id);
        reject(new Error('Request timed out; the backend operation may still be running'));
      }, 120000);
      pending.set(request.id, {
        resolve,
        reject,
        timeout,
        target,
        read,
        turn: request.type === 'agent.turn.abort' ? request.message.turn : undefined,
        expectedType: COMMAND_RESULT[request.type],
      });
      try {
        socket!.send(JSON.stringify(request));
      } catch (error) {
        clearTimeout(timeout);
        pending.delete(request.id);
        reject(error instanceof Error ? error : new Error(String(error)));
      }
    });
  }
  function executeFor(
    target: Target,
    command: string,
    argumentsValue: unknown[] = [],
  ): Promise<unknown> {
    if (!command.trim() || !Array.isArray(argumentsValue))
      return Promise.reject(new Error('Enter an MCL command and a JSON argument array'));
    return request(
      {
        type: 'mcl.execute',
        id: randomId(),
        message: {
          address: target.agent,

          command: command.trim(),
          arguments: argumentsValue,
        },
      },
      target,
      command.trim().startsWith('GET ') || command.trim() === 'CONFIG GET',
    );
  }
  function executeMclCommand(command: string, argumentsValue: unknown[] = []): Promise<unknown> {
    try {
      return executeFor(currentTarget()!, command, argumentsValue);
    } catch (error) {
      return Promise.reject(error);
    }
  }
  function resourceTargetSnapshot(target: Target | null): string {
    if (!target || !connected()) return '';
    return JSON.stringify([
      endpoint.value,
      routeRevision,
      target.workspace,
      target.agent,
      selectedAgentState.value?.tools,
      selectedAgentState.value?.sandboxes,
    ]);
  }
  function assertTarget(target: Target, snapshot: string) {
    if (resourceTargetSnapshot(currentTarget(false)) !== snapshot)
      throw new Error('Selected agent changed during resource settings update');
  }
  async function setSandbox(alias: string | null): Promise<void> {
    if (resourceWritePending.value) throw new Error('Wait for the current sandbox request');
    const target = currentTarget()!;
    const snapshot = resourceTargetSnapshot(target);
    assertTarget(target, snapshot);
    if (alias !== null && !selectedAgentState.value!.sandboxes.includes(alias))
      throw new Error('Choose an imported sandbox alias');
    const aliases = alias === null ? [] : [alias];
    resourceWritePending.value = snapshot;
    let saved = false;
    try {
      const persisted = await executeFor(target, 'CONFIG SET ? ?', ['sandbox', aliases]);
      requireUnit(persisted, 'CONFIG SET sandbox');
      saved = true;
      assertTarget(target, snapshot);
      const applied = await executeFor(target, 'EFFECT sandbox_use ?', [aliases]);
      requireUnit(applied, 'EFFECT sandbox_use');
      assertTarget(target, snapshot);
      queryState();
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      // Save and apply are separate steps; refresh the authoritative Agent
      // snapshot so the panel shows what actually runs.
      queryState();
      throw new Error(
        saved ? `Sandbox saved but not activated: ${message}` : `Sandbox was not saved: ${message}`,
      );
    } finally {
      if (resourceWritePending.value === snapshot) resourceWritePending.value = '';
    }
  }
  async function sendMessage(
    content: string,
    images: UserImage[] = [],
    onStage?: (stage: 'injecting' | 'waking') => void,
  ): Promise<unknown> {
    if (!content.trim() && !images.length) throw new Error('Message is empty');
    if (!selectedAgentIdle.value) throw new Error('The selected agent is not idle');
    const target = currentTarget()!;
    const revision = routeRevision;
    const targetKey = `${target.workspace}:${target.agent}`;
    if (sendingTargets.has(targetKey))
      throw new Error('A message is already awaiting acknowledgement for this agent');
    const text = content.trim();
    const instruction = contextInjection(text, driverContextPath.value, images);
    const queued = pendingWakes.get(targetKey);
    const alreadyInjected =
      !!queued && queued.content === text && sameImages(queued.images, images);
    sendingTargets.add(targetKey);
    let stage: 'injecting' | 'waking' = alreadyInjected ? 'waking' : 'injecting';
    onStage?.(stage);
    try {
      if (alreadyInjected) {
        // The daemon already accepted this message into entry.pending; a wake
        // failure must not inject the same text a second time.
        addLog('INFO', 'Message was already injected; retrying wake only', 'ui.send');
      } else {
        if (queued)
          addLog(
            'WARN',
            'A previously injected message is still queued; it will be consumed by this wake',
            'ui.send',
          );
        addLog('INFO', 'Sending message; waiting for daemon injection acknowledgement', 'ui.send', [
          { name: 'images', value: String(images.length) },
        ]);
        const injected = await executeFor(target, instruction.command, instruction.arguments);
        requireUnit(injected, 'INJECT ? TO entry.pending[-1]');
        const current = currentTarget(false);
        if (
          revision !== routeRevision ||
          !current ||
          current.workspace !== target.workspace ||
          current.agent !== target.agent
        )
          throw new Error('Selected agent changed during send');
        pendingWakes.set(targetKey, {
          workspace: target.workspace,
          agent: target.agent,

          content: text,
          images: images.map(({ media_type, data }) => ({ media_type, data })),
        });
        stage = 'waking';
        onStage?.(stage);
        addLog('INFO', 'Message injected; waiting for daemon wake acknowledgement', 'ui.send');
      }
      const result = await executeFor(target, 'EFFECT wake');
      requireUnit(result, 'EFFECT wake');
      pendingWakes.delete(targetKey);
      addLog('INFO', 'Daemon acknowledged message and wake', 'ui.send');
      return result;
    } catch (error) {
      addLog(
        'ERROR',
        `Message send failed while awaiting ${stage === 'injecting' ? 'injection' : 'wake'} acknowledgement; see composer error`,
        'ui.send',
      );
      throw error;
    } finally {
      sendingTargets.delete(targetKey);
    }
  }
  function remember(set: Set<string>, key: string) {
    set.add(key);
    if (set.size > 1000) set.delete(set.values().next().value!);
  }
  function abortTurn(): Promise<unknown> {
    const target = currentTarget(false);
    const state = selectedAgentState.value;
    if (!target || !state?.turn || !canAbortTurn.value)
      return Promise.reject(new Error('No abortable turn is active'));
    if (!connected()) return Promise.reject(new Error('Register a daemon connection first'));
    const id = randomId();
    const guard: AbortGuard = {
      ...target,
      id,
      turn: state.turn,
      failed: false,
      streamKeys: new Set(),
    };
    for (const [key, stream] of streams.value)
      if (
        stream.workspaceKey === target.workspace &&
        stream.agent === target.agent &&
        !stream.completed
      ) {
        guard.streamKeys.add(key);
        remember(abortedStreams, key);
        streams.value.set(key, { ...stream, frozen: true });
      }
    abortGuards.value.set(id, guard);
    return request(
      {
        type: 'agent.turn.abort',
        id,
        message: {
          address: target.agent,

          turn: state.turn,
        },
      },
      target,
    ).catch((error) => {
      const current = abortGuards.value.get(id);
      if (current) current.failed = true;
      queryState();
      throw error;
    });
  }
  function reconcileAbortGuards() {
    for (const [id, guard] of abortGuards.value) {
      const state = agentStates.value.find(
        (candidate) =>
          workspaceKey(candidate.workspace) === guard.workspace &&
          candidate.agent === guard.agent,
      );
      const sameTurn = state?.status === 'working' && state.turn === guard.turn;
      if (guard.failed && sameTurn) {
        for (const key of guard.streamKeys) {
          abortedStreams.delete(key);
          const stream = streams.value.get(key);
          if (stream) streams.value.set(key, { ...stream, frozen: false });
        }
        abortGuards.value.delete(id);
      } else if (
        !state ||
        state.status !== 'working' ||
        (state.turn !== null && state.turn !== guard.turn)
      )
        abortGuards.value.delete(id);
    }
  }
  function handleDelta(event: StreamDelta, reasoning: boolean) {
    const state = agentStates.value.find(
      (candidate) =>
        candidate.agent === event.agent,
    );
    if (!state) return;
    const key = `${event.agent}:${event.id}`;
    if (completedStreams.has(key) || abortedStreams.has(key)) return;
    const guard = [...abortGuards.value.values()].find(
      (candidate) =>
        candidate.workspace === workspaceKey(state.workspace) &&
        candidate.agent === event.agent,
    );
    if (guard) {
      guard.streamKeys.add(key);
      remember(abortedStreams, key);
      return;
    }
    const old = streams.value.get(key);
    if (old && (old.frozen || old.completed || event.sequence <= old.lastSequence)) return;
    const message: StreamEntry = old
      ? { ...old }
      : {
          key,
          id: event.id,
          workspaceKey: workspaceKey(state.workspace),
          agent: event.agent,
          role: 'assistant',
          thinking: '',
          content: '',
          toolCalls: [],
          failed: false,
          timestamp: Date.now(),
          kind: 'stream',
          payload: null,
          source: null,
          messageType: null,
          sequence: null,
          toolCallId: null,
          toolName: null,

          lastSequence: -1,
          completed: false,
          frozen: false,
        };
    if (reasoning) message.thinking += event.content;
    else message.content += event.content;
    message.lastSequence = event.sequence;
    streams.value = new Map(streams.value).set(key, message);
  }
  function settle(id: string, result: unknown, failure?: string | Error) {
    const command = pending.get(id);
    if (!command) return;
    clearTimeout(command.timeout);
    pending.delete(id);
    if (failure) command.reject(typeof failure === 'string' ? new Error(failure) : failure);
    else command.resolve(result);
  }
  function requireUnit(value: unknown, operation: string): void {
    if (value !== 'Unit')
      throw new Error(
        `${operation} did not acknowledge Unit; the outcome is unknown, inspect state before retrying`,
      );
  }
  function reconcileStreams() {
    if (!streams.value.size) return;
    const tracked = new Set<string>();
    for (const stream of streams.value.values())
      tracked.add(`${stream.workspaceKey}:${stream.agent}`);
    const assistantIds = new Set<string>();
    for (const history of histories.value) {
      const workspace = workspaceKey(history.workspace);
      if (!tracked.has(`${workspace}:${history.agent}`)) continue;
      for (const entry of history.messages) {
        if (entry.kind !== 'message') continue;
        const message = historyMessage(entry);
        if (message?.type !== 'assistant') continue;
        assistantIds.add(`${workspace}:${history.agent}:${payloadRequestId(entry)}`);
      }
    }
    streams.value = new Map(
      [...streams.value].filter(
        ([, stream]) =>
          agentStates.value.some(
            (state) =>
              state.agent === stream.agent &&
              workspaceKey(state.workspace) === stream.workspaceKey,
          ) && !assistantIds.has(`${stream.workspaceKey}:${stream.agent}:${stream.id}`),
      ),
    );
  }
  // The Agent list is the authoritative activation set: release state that
  // belongs to instances that no longer exist.
  function pruneAgentState() {
    const live = new Set(activeAgentInfos.value.map((agent) => agent.address));
    const retain = <T>(source: Map<string, T>) =>
      new Map([...source].filter(([agent]) => live.has(agent)));
    turnStates.value = retain(turnStates.value);
    taskStates.value = retain(taskStates.value);
    agentLogs.value = retain(agentLogs.value);
    mclRecords.value = retain(mclRecords.value);
    for (const [key, wake] of pendingWakes) if (!live.has(wake.agent)) pendingWakes.delete(key);
    histories.value = histories.value.filter((history) => live.has(history.agent));
  }
  function projectSnapshots() {
    definitions.value = workspaceSnapshots.value.map((w) => ({
      path: w.path,
      cache: w.definition,
    }));
    runtimeWorkspaces.value = workspaceSnapshots.value
      .filter((w) => w.address !== null)
      .map((w) => ({
        id: w.address!,
        name: w.definition.name,
        project_root: w.definition.project_root,
        definition_path: w.path,
        status: w.status as WorkspaceInfo['status'],
        error: w.error,
        agents: activeAgentInfos.value
          .filter((a) => a.workspace === w.address)
          .map((a) => a.address),
      }));
    agentStates.value = activeAgentInfos.value.map((a) => ({
      workspace: runtimeWorkspaces.value.find((w) => w.id === a.workspace) ?? {
        id: a.workspace,
        name: resourceName(a.workspace),
        project_root: '',
      },
      agent: a.address,

      status: (a.status === 'ready' ? 'idle' : a.status) as AgentState['status'],
      turn: turnStates.value.get(`${a.address}`) ?? null,
      tools: a.tools.map((t) => t.alias),
      sandbox: a.sandbox,
      sandboxes: a.sandboxes.map((sandbox) => sandbox.alias),
      queued_commands: 0,
      pending_command: null,
      context_window_tokens: a.model?.context_window_tokens ?? null,
      error: a.error,
    }));
    toolDefinitions.value = new Map(
      activeAgentInfos.value.map((a) => [`${a.address}`, a.tools]),
    );
    for (const state of agentStates.value) {
      const key = `${workspaceKey(state.workspace)}:${state.agent}`;
      if (state.error) lastFailures.value.set(key, { message: state.error.message });
      else if (['idle', 'working'].includes(state.status)) lastFailures.value.delete(key);
    }
    reconcileAbortGuards();
    reconcileSelection();
    reconcileStreams();
  }
  function mergeHistory(target: Target, records: HistoryMessage[]) {
    const previous = histories.value.find(
      (h) => h.agent === target.agent && workspaceKey(h.workspace) === target.workspace,
    );
    const rows = new Map((previous?.messages ?? []).map((row) => [row.sequence, row]));
    for (const row of records) {
      const old = rows.get(row.sequence);
      rows.set(
        row.sequence,
        row.created_at_ms === 0 && old ? { ...row, created_at_ms: old.created_at_ms } : row,
      );
    }
    const workspace = agentStates.value.find((a) => a.agent === target.agent)?.workspace;
    if (!workspace) return;
    histories.value = [
      ...histories.value.filter((h) => h !== previous),
      {
        workspace,
        agent: target.agent,
        messages: [...rows.values()].sort((a, b) => a.sequence - b.sequence),
      },
    ];
    reconcileStreams();
  }
  function queryTarget(id: string, address?: string): Target | null {
    const query = recordQueries.get(id);
    recordQueries.delete(id);
    historyQueries.delete(id);
    if (
      !query ||
      !matchesTarget(query) ||
      (address !== undefined && query.agent !== address)
    )
      return null;
    return query;
  }
  function mclValue(data: MclData | undefined): unknown {
    if (!data) return 'Unit';
    switch (data.kind) {
      case 'messages':
        return { Array: { Message: data.value } };
      case 'config':
        return { Config: data.value };
      case 'import':
        return { Import: data.value };
      case 'inference':
        return { Inference: data.value };
      case 'block_snapshots':
        return { BlockSnapshots: data.value };
    }
  }
  function handleFull(event: Extract<ServerMessage, { type: 'agent.message.full.sync' }>) {
    const state = agentStates.value.find(
      (a) => a.agent === event.address,
    );
    if (!state) return;
    const target = {
      workspace: workspaceKey(state.workspace),
      agent: event.address,

    };
    if (event.history_sequence !== null) {
      const message = event.message;
      mergeHistory(target, [
        {
          sequence: event.history_sequence,
          turn: event.turn,
          kind: 'message',
          content:
            'content' in message
              ? (message.content ?? '')
              : message.type === 'error'
                ? message.message
                : '',
          payload: message,
          created_at_ms: 0,
        },
      ]);
    }
    if (event.generation) {
      const key = `${event.address}:${event.generation}`;
      remember(completedStreams, key);
      if (event.history_sequence !== null) {
        streams.value.delete(key);
        streams.value = new Map(streams.value);
      } else {
        const row = decodeMessage(event.message)[0];
        if (row) {
          const old = streams.value.get(key);
          streams.value = new Map(streams.value).set(key, {
            key,
            id: event.generation,
            workspaceKey: target.workspace,
            agent: target.agent,
            role: row.role,
            thinking: row.thinking,
            content: row.content,
            toolCalls: row.toolCalls,
            failed: row.failed || !!event.error,
            images: row.images,
            timestamp: old?.timestamp ?? 0,
            kind: 'stream',
            payload: event.message,
            source: null,
            messageType: event.message.type,
            sequence: null,
            toolCallId: row.toolCallId,
            toolName: row.toolName,

            lastSequence: old?.lastSequence ?? -1,
            completed: true,
            frozen: old?.frozen ?? false,
          });
        }
      }
    }
    if (event.error) addLog('ERROR', event.error.message, event.address);
  }
  function handleServerMessage(event: ServerMessage) {
    if ('id' in event) {
      const command = pending.get(event.id);
      if (command?.expectedType && command.expectedType !== event.type) {
        settle(
          event.id,
          null,
          `Received ${event.type} for a waiting ${command.expectedType}`,
        );
        return;
      }
    }
    switch (event.type) {
      case 'connection.register.result':
        if (event.id !== registrationId) break;
        if (!event.success) {
          connectionError.value = event.error.message;
          shouldReconnect = false;
          socket?.close();
          break;
        }
        registeredClient.value = event.client;
        connectionStatus.value = 'online';
        connectionError.value = '';
        reconnectAttempt = 0;
        queryState();
        break;
      case 'workspace.res':
        if (listQueries.get(event.id) !== 'workspace.req') break;
        listQueries.delete(event.id);
        if (!event.success) {
          addLog('ERROR', event.error.message, 'ui.workspace');
          break;
        }
        workspaceSnapshots.value = event.workspaces;
        stateReady.value = true;
        projectSnapshots();
        settle(event.id, event.workspaces);
        break;
      case 'workspace.sync':
        workspaceSnapshots.value = event.workspaces;
        stateReady.value = true;
        projectSnapshots();
        break;
      case 'agent.res':
        if (listQueries.get(event.id) !== 'agent.req') break;
        listQueries.delete(event.id);
        if (!event.success) {
          addLog('ERROR', event.error.message, 'ui.agent');
          break;
        }
        activeAgentInfos.value = event.agents;
        pruneAgentState();
        projectSnapshots();
        settle(event.id, event.agents);
        break;
      case 'agent.sync':
        activeAgentInfos.value = event.agents;
        pruneAgentState();
        projectSnapshots();
        break;
      case 'workspace.start.result':
      case 'workspace.stop.result':
      case 'workspace.restart.result':
      case 'workspace.reload.result':
      case 'agent.start.result':
      case 'agent.stop.result':
      case 'agent.restart.result':
        settle(event.id, null, event.success ? undefined : event.error.message);
        break;
      case 'agent.turn.abort.result': {
        if (!pending.has(event.id)) break;
        const guard = abortGuards.value.get(event.id);
        if (!event.success && guard) guard.failed = true;
        settle(event.id, null, event.success ? undefined : event.error.message);
        if (!event.success) queryState();
        break;
      }
      case 'agent.turn.res': {
        const query = queryTarget(event.id);
        if (!query) break;
        if (!event.success) {
          addLog('ERROR', event.error.message);
          break;
        }
        if (query.agent !== event.state.address) break;
        turnStates.value.set(targetKey(query), event.state.turn);
        projectSnapshots();
        break;
      }
      case 'agent.turn.sync':
        if (matchesTarget({ agent: event.address })) {
          turnStates.value.set(`${event.address}`, event.turn);
          projectSnapshots();
        }
        break;
      case 'agent.task.res': {
        if (!queryTarget(event.id, event.address)) break;
        if (event.success)
          taskStates.value = new Map(taskStates.value).set(
            `${event.address}`,
            event.tasks,
          );
        else addLog('ERROR', event.error.message);
        break;
      }
      case 'agent.task.sync':
        if (matchesTarget({ agent: event.address }))
          taskStates.value = new Map(taskStates.value).set(
            `${event.address}`,
            event.tasks,
          );
        break;
      case 'mcl.execute.result': {
        const command = pending.get(event.id);
        if (!command) break;
        if (command.target && !matchesTarget(command.target)) {
          settle(event.id, null, 'Agent instance changed before the command result');
          break;
        }
        settle(
          event.id,
          event.success ? mclValue(event.data) : null,
          event.success ? undefined : event.error.message,
        );
        break;
      }
      case 'agent.history.res': {
        const query = queryTarget(event.id, event.address);
        if (!query) break;
        if (!event.success) {
          addLog('ERROR', event.error.message);
          break;
        }
        mergeHistory(query, event.records);
        historyCursors.value = new Map(historyCursors.value).set(targetKey(query), {
          before: event.next_before_sequence,
          after: event.next_after_sequence,
        });
        break;
      }
      case 'agent.log.res':
      case 'agent.mcl.res': {
        const query = queryTarget(event.id, event.address);
        if (!query) break;
        if (!event.success) {
          addLog('ERROR', event.error.message);
          break;
        }
        if (event.type === 'agent.log.res') mergeLogs(query, event.records);
        else mergeAudits(query, event.records);
        break;
      }
      case 'agent.log.sync':
        if (matchesTarget({ agent: event.address }))
          mergeLogs({ agent: event.address }, event.records);
        break;
      case 'agent.mcl.sync':
        if (matchesTarget({ agent: event.address }))
          mergeAudits({ agent: event.address }, event.records);
        break;
      case 'agent.message.chunk.sync':
        if (
          (streams.value.get(`${event.address}:${event.generation}`)
            ?.lastSequence ?? -1) >= event.sequence
        )
          break;
        if (event.delta.reasoning !== undefined)
          handleDelta(
            {
              id: event.generation,
              agent: event.address,

              sequence: event.sequence,
              content: event.delta.reasoning,
            },
            true,
          );
        if (event.delta.content !== undefined) {
          // A single chunk may contain both fields; append content within the same accepted sequence.
          const key = `${event.address}:${event.generation}`;
          if (event.delta.reasoning !== undefined) {
            const stream = streams.value.get(key);
            if (
              stream &&
              stream.lastSequence === event.sequence &&
              !stream.frozen &&
              !stream.completed
            )
              streams.value = new Map(streams.value).set(key, {
                ...stream,
                content: stream.content + event.delta.content,
              });
          } else
            handleDelta(
              {
                id: event.generation,
                agent: event.address,

                sequence: event.sequence,
                content: event.delta.content,
              },
              false,
            );
        }
        break;
      case 'agent.message.full.sync':
        handleFull(event);
        break;
    }
  }
  function mergeLogs(target: Pick<Target, 'agent'>, records: AgentLogRecord[]) {
    const key = targetKey(target);
    const rows = new Map((agentLogs.value.get(key) ?? []).map((r) => [r.sequence, r]));
    for (const row of records) rows.set(row.sequence, row);
    agentLogs.value = new Map(agentLogs.value).set(
      key,
      [...rows.values()].sort((a, b) => a.sequence - b.sequence),
    );
  }
  function mergeAudits(target: Pick<Target, 'agent'>, records: AgentMclRecord[]) {
    const key = targetKey(target);
    const rows = new Map((mclRecords.value.get(key) ?? []).map((r) => [r.sequence, r]));
    for (const row of records) rows.set(row.sequence, row);
    mclRecords.value = new Map(mclRecords.value).set(
      key,
      [...rows.values()].sort((a, b) => a.sequence - b.sequence),
    );
  }
  async function workspaceRequest(message: ClientMessage) {
    if (workspaceOperationPending.value)
      throw new Error('Wait for the current workspace operation');
    workspaceOperationPending.value = true;
    workspaceNotice.value = '';
    try {
      return await request(message);
    } catch (error) {
      workspaceNotice.value = error instanceof Error ? error.message : String(error);
      throw error;
    } finally {
      workspaceOperationPending.value = false;
    }
  }
  async function startWorkspace(path = selectedDefinition.value?.path): Promise<void> {
    if (!path || !definitions.value.some((entry) => entry.path === path))
      throw new Error('Choose a configured workspace definition');
    if (runtimeWorkspaces.value.some((workspace) => workspace.definition_path === path))
      throw new Error('Workspace is already active');
    await workspaceRequest({ type: 'workspace.start', id: randomId(), message: { path } });
    workspaceNotice.value = 'Workspace started.';
  }
  async function restartWorkspace(path = selectedDefinition.value?.path): Promise<void> {
    if (
      !path ||
      !runtimeWorkspaces.value.some(
        (workspace) => workspace.definition_path === path && workspace.status === 'running',
      )
    )
      throw new Error('Choose a Running workspace');
    await workspaceRequest({
      type: 'workspace.restart',
      id: randomId(),
      message: { path },
    });
    workspaceNotice.value = 'Workspace restarted.';
  }
  async function stopWorkspace(address = selectedRuntimeWorkspace.value?.id): Promise<void> {
    if (
      !address ||
      !runtimeWorkspaces.value.some(
        (workspace) => workspace.id === address && workspace.status === 'running',
      )
    )
      throw new Error('Choose a Running workspace');
    await workspaceRequest({
      type: 'workspace.stop',
      id: randomId(),
      message: { path: runtimeWorkspaces.value.find((w) => w.id === address)!.definition_path },
    });
    workspaceNotice.value = 'Workspace stopped.';
  }
  function clearLogs() {
    logs.value = [];
  }
  return {
    workspaceSnapshots,
    activeAgentInfos,
    taskStates,
    toolDefinitions,
    agentLogs,
    mclRecords,
    historyCursors,
    selectedMclEntries,
    selectedSessionLogs,
    loadOlderHistory,
    queryRecords,
    endpoint,
    driverContextPath,
    connectionStatus,
    registeredClient,
    stateReady,
    connectionError,
    definitions,
    runtimeWorkspaces,
    workspaces,
    selectedDefinition,
    selectedRuntimeWorkspace,
    workspaceOperationPending,
    workspaceNotice,
    startWorkspace,
    restartWorkspace,
    stopWorkspace,
    sidebarTree,
    toggleWorkspace,
    selectedWorkspaceKey,
    selectedWorkspace,
    selectedAgent,
    selectedAgentName,
    selectedAgentState,
    selectedAgentReady,
    selectedAgentIdle,
    selectedAgentWorking,
    abortPending,
    canAbortTurn,
    abortTurn,
    lastFailures,
    visibleMessages,
    tokenUsage,
    contextTokens,
    contextWindowTokens,
    logs,
    connect,
    disconnect,
    selectWorkspace,
    selectAgent,
    sendMessage,
    executeMclCommand,
    toolOptions,
    sandboxOptions,
    resourceWriteInFlight,
    setSandbox,
    clearLogs,
    handleServerMessage,
  };
});
