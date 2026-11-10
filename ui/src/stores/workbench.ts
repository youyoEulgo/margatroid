import { computed, ref } from 'vue';
import { defineStore } from 'pinia';
import {
  parseServerMessage,
  isRecord,
  isAgentMessage,
  isMclError,
  errorMessage,
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
} from '@/types/protocol';
import { randomId } from '@/lib/id';

export type ConnectionStatus = 'offline' | 'connecting' | 'online';
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
  instance: string;
  lastSequence: number;
  completed: boolean;
  frozen: boolean;
}
interface Target {
  workspace: string;
  agent: string;
  instance: string;
}
interface PendingCommand {
  resolve: (value: unknown) => void;
  reject: (error: Error) => void;
  timeout: ReturnType<typeof setTimeout>;
  target?: Target;
  read?: boolean;
  turn?: string;
}
interface MclCommandFailure extends Error {
  requestId?: string;
  agentMessage?: unknown;
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
        agents.find((agent) => agent.reason)?.reason || instance?.member_failures?.[0]?.error || '';
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
    { workspace: string; agent: string; instance: string; content: string; images: UserImage[] }
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
      !!selectedAgentState.value?.agent_instance &&
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
        guard.agent === selectedAgent.value &&
        guard.instance === selectedAgentState.value?.agent_instance,
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
  function queryState() {
    if (connected())
      socket!.send(
        JSON.stringify({
          type: 'state.query',
          id: randomId(),
          message: {},
        } satisfies ClientMessage),
      );
  }
  function queryHistoryFor(target: Pick<Target, 'workspace' | 'agent'>) {
    if (connected())
      socket!.send(
        JSON.stringify({
          type: 'history.query',
          id: randomId(),
          message: { workspace: target.workspace, agent: target.agent },
        } satisfies ClientMessage),
      );
  }
  function queryHistory() {
    const target = currentTarget(false);
    if (target) queryHistoryFor(target);
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
      if (socket === next)
        next.send(
          JSON.stringify({
            type: 'connection.register',
            id: randomId(),
            message: { client_type: 'webui' },
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
    return { workspace: workspace.id, agent: selectedAgent.value, instance: state.agent_instance };
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
        turn: request.type === 'agent.turn.abort' ? request.message.turn_id : undefined,
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
        type: 'mcl.command',
        id: randomId(),
        message: {
          workspace: target.workspace,
          agent: target.agent,
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
      target.instance,
      selectedAgentState.value?.tools,
      selectedAgentState.value?.sandboxes,
    ]);
  }
  function assertTarget(target: Target, snapshot: string) {
    if (resourceTargetSnapshot(currentTarget(false)) !== snapshot)
      throw new Error('Selected agent changed during resource settings update');
    if (selectedAgentState.value?.agent_instance !== target.instance)
      throw new Error('Selected agent instance changed during resource settings update');
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
      // Saved and applied are separate steps: re-read both so the panel shows
      // the actual persisted value and the actual running sandbox.
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
    const targetKey = `${target.workspace}:${target.agent}:${target.instance}`;
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
          current.agent !== target.agent ||
          current.instance !== target.instance
        )
          throw new Error('Selected agent changed during send');
        pendingWakes.set(targetKey, {
          workspace: target.workspace,
          agent: target.agent,
          instance: target.instance,
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
        stream.instance === target.instance &&
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
          workspace: target.workspace,
          agent: target.agent,
          agent_instance: target.instance,
          turn_id: state.turn,
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
          candidate.agent === guard.agent &&
          candidate.agent_instance === guard.instance,
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
        candidate.agent === event.agent && candidate.agent_instance === event.agent_instance,
    );
    if (!state) return;
    const key = `${event.agent}:${event.agent_instance}:${event.id}`;
    if (completedStreams.has(key) || abortedStreams.has(key)) return;
    const guard = [...abortGuards.value.values()].find(
      (candidate) =>
        candidate.workspace === workspaceKey(state.workspace) &&
        candidate.agent === event.agent &&
        candidate.instance === event.agent_instance,
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
          instance: event.agent_instance,
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
              workspaceKey(state.workspace) === stream.workspaceKey &&
              state.agent_instance === stream.instance,
          ) && !assistantIds.has(`${stream.workspaceKey}:${stream.agent}:${stream.id}`),
      ),
    );
  }
  function handleServerMessage(event: ServerMessage) {
    switch (event.type) {
      case 'connection.registered':
        registeredClient.value = event.client;
        connectionStatus.value = 'online';
        connectionError.value = '';
        reconnectAttempt = 0;
        queryState();
        break;
      case 'connection.register_failed':
        connectionError.value = errorMessage(event.error);
        shouldReconnect = false;
        socket?.close();
        break;
      case 'state.sync':
        definitions.value = event.state.workspace_definitions;
        runtimeWorkspaces.value = event.state.workspaces;
        agentStates.value = event.state.agents;
        histories.value = event.state.histories;
        stateReady.value = true;
        reconcileAbortGuards();
        for (const [key, wake] of pendingWakes)
          if (
            !agentStates.value.some(
              (state) =>
                workspaceKey(state.workspace) === wake.workspace &&
                state.agent === wake.agent &&
                state.agent_instance === wake.instance &&
                state.status === 'idle',
            )
          )
            pendingWakes.delete(key);
        for (const state of agentStates.value) {
          const key = `${workspaceKey(state.workspace)}:${state.agent}`;
          if (state.error) lastFailures.value.set(key, { message: state.error.message });
          else if (['idle', 'working'].includes(state.status)) lastFailures.value.delete(key);
        }
        reconcileSelection();
        reconcileStreams();
        break;
      case 'workspace.definitions':
        definitions.value = event.definitions;
        reconcileSelection();
        settle(event.id, event.definitions);
        break;
      case 'workspace.started':
      case 'workspace.restarted':
        runtimeWorkspaces.value = [
          ...runtimeWorkspaces.value.filter((workspace) => workspace.id !== event.workspace.id),
          event.workspace,
        ];
        for (const failure of event.workspace.member_failures ?? [])
          addLog(
            'ERROR',
            `Agent ${failure.agent} (${failure.driver}) failed to start: ${failure.error}`,
          );
        settle(event.id, event.workspace);
        queryState();
        break;
      case 'workspace.stopped': {
        const stopped = workspaceKey(event.workspace);
        const wasSelected =
          !!selectedRuntimeWorkspace.value && selectedRuntimeWorkspace.value.id === stopped;
        settle(event.id, event.workspace);
        // The receipt closes the operation; the runtime list still follows the
        // next authoritative snapshot, but this instance's selection, streams
        // and pending routes are no longer usable.
        streams.value = new Map(
          [...streams.value].filter(([, stream]) => stream.workspaceKey !== stopped),
        );
        for (const [key, guard] of abortGuards.value)
          if (guard.workspace === stopped) abortGuards.value.delete(key);
        for (const [key, wake] of pendingWakes)
          if (wake.workspace === stopped) pendingWakes.delete(key);
        if (wasSelected) {
          routeRevision++;
          selectedAgent.value = null;
          resourcesReset();
        }
        queryState();
        break;
      }
      case 'workspace.start_failed':
      case 'workspace.restart_failed':
      case 'workspace.stop_failed':
        settle(event.id, null, event.error.message);
        queryState();
        break;
      case 'agent.started':
      case 'agent.restarted':
        settle(event.id, event.agent);
        queryState();
        break;
      case 'agent.turn.abort_result': {
        const command = pending.get(event.id);
        const guard = abortGuards.value.get(event.id);
        if (
          !command?.target ||
          command.target.workspace !== workspaceKey(event.workspace) ||
          command.target.agent !== event.agent ||
          command.turn !== event.turn_id
        )
          break;
        if ('Err' in event.result && guard) guard.failed = true;
        settle(event.id, null, 'Err' in event.result ? event.result.Err.message : undefined);
        queryState();
        break;
      }
      case 'mcl.command_result': {
        const command = pending.get(event.id);
        if (!command) break;
        if (command.target?.agent !== event.target) {
          settle(event.id, null, 'MCL result target does not match the request');
          break;
        }
        const value = event.result.Ok;
        let failure: MclCommandFailure | undefined;
        if (isRecord(value) && 'Error' in value && isMclError(value.Error)) {
          failure = new Error(value.Error.message);
          failure.requestId = value.Error.request_id;
          failure.agentMessage = value.Error;
        }
        settle(event.id, value, failure);
        if (!command.read) queryHistoryFor(command.target);
        if (event.history_error)
          addLog(
            'ERROR',
            `Command completed but history persistence failed: ${event.history_error.message}`,
          );
        break;
      }
      case 'request.failed': {
        const guard = abortGuards.value.get(event.id);
        if (guard) guard.failed = true;
        if (pending.has(event.id)) settle(event.id, null, event.error.message);
        else addLog('ERROR', event.error.message);
        queryState();
        break;
      }
      case 'agent.message.delta':
        handleDelta(event, false);
        break;
      case 'agent.message.reasoning_delta':
        handleDelta(event, true);
        break;
      case 'agent.message.finished': {
        const state = agentStates.value.find(
          (candidate) =>
            candidate.agent === event.agent && candidate.agent_instance === event.agent_instance,
        );
        if (!state) break;
        const key = `${event.agent}:${event.agent_instance}:${event.id}`;
        if (completedStreams.has(key)) break;
        remember(completedStreams, key);
        const old = streams.value.get(key);
        if (old)
          streams.value = new Map(streams.value).set(key, {
            ...old,
            completed: true,
            failed: !!event.error,
          });
        if (event.error) {
          lastFailures.value.set(`${workspaceKey(state.workspace)}:${event.agent}`, {
            message: event.error.message,
          });
          addLog('ERROR', event.error.message, event.agent);
        }
        queryHistoryFor({ workspace: workspaceKey(state.workspace), agent: state.agent });
        break;
      }
      case 'history.result':
        histories.value = [
          ...histories.value.filter(
            (history) =>
              history.agent !== event.history.agent ||
              workspaceKey(history.workspace) !== workspaceKey(event.history.workspace),
          ),
          event.history,
        ];
        reconcileStreams();
        break;
      case 'log':
        logs.value.push({
          key: randomId(),
          timestamp: event.record.timestamp_millis,
          level: event.record.level,
          target: event.record.target,
          message: event.record.message,
          fields: event.record.fields,
        });
        if (logs.value.length > MAX_LOGS) logs.value.splice(0, logs.value.length - MAX_LOGS);
        break;
    }
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
      message: { workspace: address },
    });
    workspaceNotice.value = 'Workspace stopped.';
  }
  function clearLogs() {
    logs.value = [];
  }
  return {
    endpoint,
    driverContextPath,
    connectionStatus,
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
