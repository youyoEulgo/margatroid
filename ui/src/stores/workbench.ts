import { computed, ref } from 'vue';
import { defineStore } from 'pinia';
import {
  parseServerMessage,
  isRecord,
  isAgentMessage,
  errorMessage,
  CATALOG_REQUESTS,
  type CatalogEntry,
  type DriverCatalog,
  type WorkspacePaths,
  type CatalogRequestType,
  type AgentHistory,
  type AgentMessage,
  type AgentState,
  type ClientInfo,
  type ClientMessage,
  type HistoryMessage,
  type ServerMessage,
  type StreamDelta,
  type ToolCall,
  type WorkspaceInfo,
  type WorkspaceRef,
  type UserImage,
} from '@/types/protocol';

export type ConnectionStatus = 'offline' | 'connecting' | 'online';
export type MessageRole = 'system' | 'user' | 'assistant' | 'tool' | 'error' | 'record';
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
}
export interface RuntimeLog {
  key: string;
  timestamp: number;
  level: string;
  target: string;
  message: string;
  fields: Array<{ name: string; value: string }>;
}
interface StreamEntry extends ConversationEntry {
  instance: string;
  lastSequence: number;
  completed: boolean;
}
interface PendingCommand {
  resolve: (value: unknown) => void;
  reject: (error: Error) => void;
  timeout: ReturnType<typeof setTimeout>;
}
const MAX_LOGS = 500;
function defaultEndpoint(): string {
  if (typeof window === 'undefined') return 'ws://127.0.0.1:3939/ws';
  return `${window.location.protocol === 'https:' ? 'wss:' : 'ws:'}//${window.location.host}/ws`;
}
export function workspaceKey(workspace: WorkspaceRef): string {
  return workspace.id;
}
export function resourceName(resource: string): string {
  return resource.split('/').pop()?.split(':')[0] || resource;
}
export function contextInjection(
  content: string,
  path: string,
  images: UserImage[] = [],
): { command: string; arguments: unknown[][] } {
  if (!path.trim() || !/^[A-Za-z_][A-Za-z0-9_.[\]-]*$/.test(path.trim()))
    throw new Error('Configure a valid driver context path before injecting a message');
  return { command: `INJECT ? TO ${path.trim()}`, arguments: [[{ type: 'user', content,
    ...(images.length ? { images: images.map(({ media_type, data }) => ({ media_type, data })) } : {}) }]] };
}
export function parseArguments(raw: string): unknown[][] {
  const value: unknown = JSON.parse(raw);
  if (!Array.isArray(value) || !value.every(Array.isArray))
    throw new Error('Arguments must be a JSON array of arrays, one array per ? placeholder');
  return value;
}
export function historyEntries(history: AgentHistory): ConversationEntry[] {
  return history.messages.flatMap((entry) => {
    const base = {
      key: `${workspaceKey(history.workspace)}:${history.agent}:${entry.key}`,
      id: payloadRequestId(entry),
      workspaceKey: workspaceKey(history.workspace),
      agent: history.agent,
      timestamp: entry.created_at_ms,
      kind: entry.kind,
      payload: entry.payload,
    };
    const payload = historyMessage(entry);
    if (payload)
      return decodeMessage(payload).map((message, index) => ({
        ...base,
        ...message,
        key: `${base.key}:${index}`,
      }));
    return [
      {
        ...base,
        role: 'record' as const,
        thinking: '',
        content: entry.content,
        toolCalls: [],
        failed: false,
      },
    ];
  });
}
export function historyMessage(entry: HistoryMessage): AgentMessage | null {
  const payload =
    isRecord(entry.payload) && isRecord(entry.payload.message)
      ? entry.payload.message
      : entry.payload;
  if (isAgentMessage(payload)) return payload;
  if (entry.kind !== 'message' || !isRecord(payload)) return null;
  const message =
    payload.type === 'error'
      ? { ...payload, message: entry.content }
      : { ...payload, content: entry.content };
  return isAgentMessage(message) ? message : null;
}

export function latestContextTokens(history: AgentHistory | undefined): number | null {
  let latest: number | null = null;
  function visit(message: AgentMessage) {
    if (message.type === 'inject') {
      for (const nested of message.messages) visit(nested);
    } else if (message.type === 'assistant') {
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
  for (const row of history?.messages ?? []) {
    if (row.kind !== 'message') continue;
    const message = historyMessage(row);
    if (message) visit(message);
  }
  return latest;
}

function payloadRequestId(entry: HistoryMessage): string {
  return isRecord(entry.payload) && typeof entry.payload.id === 'string'
    ? entry.payload.id
    : entry.key;
}
function decodeMessage(
  message: AgentMessage,
): Array<
  Pick<ConversationEntry, 'role' | 'thinking' | 'content' | 'toolCalls' | 'failed' | 'images'>
> {
  switch (message.type) {
    case 'inject':
      return message.messages.flatMap(decodeMessage);
    case 'system':
    case 'user':
      return [
        {
          role: message.type,
          images: message.type === 'user' ? message.images : undefined,
          thinking: '',
          content: message.content,
          toolCalls: [],
          failed: false,
        },
      ];
    case 'assistant':
      return [
        {
          role: 'assistant',
          thinking: message.reasoning ?? '',
          content: message.content ?? '',
          toolCalls: message.tool_calls,
          failed: false,
        },
      ];
    case 'tool_call':
      return [
        {
          role: 'tool',
          thinking: '',
          content: message.content,
          images: message.images,
          toolCalls: [],
          failed: message.failed,
        },
      ];
    case 'error':
      return [
        { role: 'error', thinking: '', content: message.message, toolCalls: [], failed: true },
      ];
  }
}
/** Sidebar status dot: gray stopped, green healthy running, red needs attention. */
export type TreeStatus = 'stopped' | 'running' | 'error';
export interface SidebarAgent {
  id: string;
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
  revision: string;
  projectRoot: string;
  status: TreeStatus;
  lifecycle: string;
  reason: string;
  /** Definition cannot be read or is invalid; creation controls stay disabled. */
  invalid: boolean;
  active: boolean;
  expanded: boolean;
  agents: SidebarAgent[];
}
export interface SidebarModel {
  workspaces: SidebarWorkspace[];
  /** No expandable workspace exists; the model still renders the workspace list. */
  selectedKey: string;
}
/**
 * Pending catalog wire. Each entry flips to `true` only after the daemon
 * implements it; until then the matching UI action reports the gap instead of
 * sending an unknown request or mutating the catalog optimistically.
 */
export const SUPPORTED_CATALOG_REQUESTS: Record<CatalogRequestType, boolean> = {
  [CATALOG_REQUESTS.createWorkspace]: true,
  [CATALOG_REQUESTS.renameWorkspace]: true,
  [CATALOG_REQUESTS.deleteWorkspace]: true,
  [CATALOG_REQUESTS.renameMember]: true,
};
const LIVE_MEMBER_STATUSES = ['creating', 'idle', 'working', 'stopping'];
function liveAgentStatus(state: AgentState | undefined): 'stopped' | 'starting' | 'live' | 'failing' {
  if (!state) return 'stopped';
  if (state.error) return 'failing';
  if (!LIVE_MEMBER_STATUSES.includes(state.status)) return 'stopped';
  return state.status === 'creating' || state.status === 'stopping' ? 'starting' : 'live';
}
/**
 * Derives the expandable sidebar. Runtime health never invents success: a
 * running workspace is green only when every defined member has a live,
 * error-free runtime state and no member failure was reported.
 */
export function buildSidebarModel(
  catalog: CatalogEntry[],
  runtime: WorkspaceInfo[],
  states: AgentState[],
  failures: Map<string, { message: string }>,
  expanded: Set<string>,
  selectedKey: string,
  selectedAgent: string | null,
): SidebarModel {
  return {
    selectedKey,
    workspaces: catalog.map((entry) => {
      const instance = runtime.find((workspace) => workspace.id === entry.id);
      const members = entry.definition?.agents ?? [];
      const memberFailures = instance?.member_failures ?? [];
      const failureFor = (agent: string) =>
        memberFailures.find((failure) => failure.agent === agent)?.error ??
        failures.get(`${entry.id}:${agent}`)?.message ??
        '';
      const agents: SidebarAgent[] = members.map((agent) => {
        const state = states.find((candidate) => candidate.workspace.id === entry.id && candidate.agent === agent.id);
        const failure = failureFor(agent.id);
        const live = liveAgentStatus(state);
        const reason = failure || (state?.error ? errorMessage(state.error) : '');
        const status: TreeStatus =
          failure || live === 'failing' ? 'error' : live === 'live' ? 'running' : 'stopped';
        return {
          id: agent.id,
          name: agent.name || resourceName(agent.id),
          driver: agent.driver,
          status,
          reason,
          active: entry.id === selectedKey && agent.id === selectedAgent,
        };
      });
      let status: TreeStatus = 'stopped';
      let reason = entry.error ?? '';
      if (entry.error) status = 'error';
      else if (entry.lifecycle === 'failed') {
        status = 'error';
        reason = 'Workspace runtime failed';
      } else {
        const unhealthy = agents.find((agent) => agent.status === 'error');
        if (unhealthy) { status = 'error'; reason = unhealthy.reason; }
        else if (entry.lifecycle === 'running' && instance && agents.every((agent) => agent.status === 'running')) status = 'running';
      }

      return {
        key: entry.id,
        name: entry.name,
        path: entry.path,
        revision: entry.revision,
        projectRoot: entry.definition?.project_root ?? '',
        status,
        lifecycle: entry.lifecycle,
        reason,
        invalid: !!entry.error || !entry.definition,
        active: entry.id === selectedKey,
        expanded: expanded.has(entry.id),
        agents,
      };
    }),
  };
}

export const useWorkbenchStore = defineStore('workbench', () => {
  const endpoint = ref(defaultEndpoint());
  const driverContextPath = ref('entry.pending[-1]');
  const lastFailures = ref(new Map<string, { message: string }>());
  const driverResources = ref(
    new Map<
      string,
      {
        imports: Array<{ resource_id: string; alias: string; kind: string }>;
        visible: Array<{ resource_id: string; alias: string }>;
        activated: string[];
      }
    >(),
  );
  const abortRequests = ref(
    new Map<string, { workspace: string; agent: string; turn: string; instance: string; streamKeys: string[] }>(),
  );
  const abortGuards = new Map<string, { workspace: string; agent: string; turn: string; instance: string; streamKeys: string[] }>();
  const connectionStatus = ref<ConnectionStatus>('offline');
  const connectionError = ref('');
  const registeredClient = ref<ClientInfo | null>(null);
  const runtimeWorkspaces = ref<WorkspaceInfo[]>([]);
  const catalog = ref<CatalogEntry[]>([]);
  /** Driver listing from `state.sync`; undefined until the daemon ships it. */
  const advertisedDrivers = ref<string[] | null>(null);
  const queriedDrivers = ref<string[] | null>(null);
  const driverCatalogErrors = ref<DriverCatalog['errors']>([]);
  const expandedWorkspaceKeys = ref(new Set<string>());
  const workspaces = computed(() => catalog.value.map((entry) => ({
    id: entry.id, name: entry.name, project_root: entry.definition?.project_root ?? entry.path,
    agents: entry.definition?.agents.map((agent) => agent.id) ?? runtimeWorkspaces.value.find((workspace) => workspace.id === entry.id)?.agents ?? [],
  })));
  const selectedCatalogEntry = computed(() => catalog.value.find((entry) => entry.id === selectedWorkspaceKey.value));
  const workspaceOperationPending = ref(false);
  const workspaceNotice = ref('');
  const agentStates = ref<AgentState[]>([]);
  const histories = ref<AgentHistory[]>([]);
  const selectedWorkspaceKey = ref('');
  const selectedAgent = ref<string | null>(null);
  let selectionInitialized = false;
  let autoSelectSoleAgent = true;
  const drivers = computed(() => [...new Set(queriedDrivers.value ?? advertisedDrivers.value ?? [])]
    .filter((driver) => /^driver:[^/\s]+\/[^:\s]+:[^\s]+$/.test(driver)).sort());
  const sidebarTree = computed<SidebarModel>(() =>
    buildSidebarModel(
      catalog.value,
      runtimeWorkspaces.value,
      agentStates.value,
      lastFailures.value,
      expandedWorkspaceKeys.value,
      selectedWorkspaceKey.value,
      selectedAgent.value,
    ),
  );
  const logs = ref<RuntimeLog[]>([]);
  const streams = ref(new Map<string, StreamEntry>());
  const pending = new Map<string, PendingCommand>();
  const renamingWorkspaces = new Set<string>();
  const renamingMembers = new Set<string>();
  const completedStreams = new Set<string>();
  // Keep abort suppression separate from completion: finished/history still reconcile partial output.
  const abortedStreams = new Set<string>();
  const readCommands = new Set<string>();
  let socket: WebSocket | null = null;
  let routeRevision = 0;
  let reconnectTimer: ReturnType<typeof setTimeout> | undefined;
  let reconnectAttempt = 0;
  let shouldReconnect = true;
  const selectedWorkspace = computed(
    () => workspaces.value.find((w) => workspaceKey(w) === selectedWorkspaceKey.value) ?? null,
  );
  const selectedAgentId = computed(
    () => selectedAgent.value || '',
  );
  const selectedAgentName = computed(() => resourceName(selectedAgentId.value) || 'Choose an agent');
  const selectedAgentState = computed(
    () =>
      agentStates.value.find(
        (state) =>
          state.workspace.id === selectedWorkspace.value?.id &&
          state.agent === selectedAgentId.value,
      ) ?? null,
  );
  const selectedAgentReady = computed(() =>
    ['idle', 'working'].includes(selectedAgentState.value?.status ?? ''),
  );
  const selectedAgentIdle = computed(() => selectedAgentState.value?.status === 'idle');
  const selectedAgentWorking = computed(() => selectedAgentState.value?.status === 'working');
  const abortPending = computed(
    () =>
      selectedAgentWorking.value &&
      (!selectedAgentState.value?.turn ||
        [...abortRequests.value.values()].some(
          (request) =>
            request.workspace === selectedWorkspaceKey.value &&
            request.agent === selectedAgentId.value &&
            request.instance === selectedAgentState.value?.agent_instance &&
            request.turn === selectedAgentState.value?.turn,
        )),
  );
  const canAbortTurn = computed(
    () => selectedAgentWorking.value && !!selectedAgentState.value?.turn && !abortPending.value,
  );
  const driverKey = computed(() =>
    selectedAgentState.value
      ? `${selectedWorkspaceKey.value}:${selectedAgentId.value}:${selectedAgentState.value.agent_instance}`
      : '',
  );
  const driverState = computed(() => driverResources.value.get(driverKey.value));
  const loadedResources = computed(
    () => driverState.value?.imports.map((item) => item.resource_id) ?? [],
  );
  const resourceVisibility = computed(() => {
    const state = driverState.value;
    if (!state) return [];
    const visible = new Set(state.visible.map((item) => item.resource_id));
    return [
      ...new Set([
        ...state.imports.filter((item) => item.kind === 'Tool').map((item) => item.resource_id),
        ...visible,
      ]),
    ]
      .sort()
      .map((resource) => ({ resource, visible: visible.has(resource), default: false }));
  });
  const sandboxOptions = computed(() => ({
    options:
      driverState.value?.imports
        .filter((item) => item.kind === 'Sandbox')
        .map((item) => item.resource_id)
        .sort() ?? [],
    activated: driverState.value?.activated ?? [],
  }));
  const selectedHistory = computed(() =>
    histories.value.find(
      (h) => h.workspace.id === selectedWorkspace.value?.id && h.agent === selectedAgentId.value,
    ),
  );
  const visibleMessages = computed(() => {
    const history = selectedHistory.value ? historyEntries(selectedHistory.value) : [];
    const live = [...streams.value.values()].filter(
      (s) => s.workspaceKey === selectedWorkspaceKey.value && s.agent === selectedAgentId.value,
    );
    return [
      ...history,
      ...live.filter((s) => !history.some((h) => h.id === s.id && h.role === 'assistant')),
    ];
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
      const value = isRecord(row.payload) ? row.payload : null;
      const message = value && isRecord(value.message) ? value.message : value;
      const usage = message && isRecord(message.usage) ? message.usage : null;
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
      key: crypto.randomUUID(),
      timestamp: Date.now(),
      level,
      message,
      target,
      fields,
    });
    if (logs.value.length > MAX_LOGS) logs.value.splice(0, logs.value.length - MAX_LOGS);
  }
  function rejectPending(message: string) {
    for (const command of pending.values()) {
      clearTimeout(command.timeout);
      command.reject(new Error(message));
    }
    pending.clear();
    abortRequests.value.clear();
  }
  function closeSocket() {
    if (socket) {
      socket.onclose = null;
      socket.close();
      socket = null;
    }
    registeredClient.value = null;
    queriedDrivers.value = null;
    driverCatalogErrors.value = [];
    streams.value = new Map();
    rejectPending('Backend WebSocket closed');
  }
  function connect(rawEndpoint?: string): boolean {
    if (rawEndpoint !== undefined) {
      try {
        endpoint.value = normalizeEndpoint(rawEndpoint);
      } catch (error) {
        connectionError.value = errorMessage(error instanceof Error ? error.message : error);
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
      const request: ClientMessage = {
        type: 'connection.register',
        id: crypto.randomUUID(),
        message: { client_type: 'webui' },
      };
      next.send(JSON.stringify(request));
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
      connectionStatus.value = 'offline';
      registeredClient.value = null;
      streams.value = new Map();
      rejectPending('Backend WebSocket disconnected');
      if (shouldReconnect) {
        const delay = Math.min(1000 * 2 ** reconnectAttempt++, 10000);
        reconnectTimer = setTimeout(() => connect(), delay);
      }
    };
    return true;
  }
  function disconnect() {
    shouldReconnect = false;
    clearTimeout(reconnectTimer);
    closeSocket();
    connectionStatus.value = 'offline';
  }
  function queryHistory() {
    const workspace = selectedWorkspace.value;
    if (workspace && selectedAgent.value && selectedAgentReady.value && socket?.readyState === WebSocket.OPEN && registeredClient.value) {
      const request: ClientMessage = {
        type: 'history.query',
        id: crypto.randomUUID(),
        message: { workspace, agent: selectedAgent.value },
      };
      socket.send(JSON.stringify(request));
    }
  }
  function selectWorkspace(key: string) {
    selectionInitialized = true;
    autoSelectSoleAgent = true;
    routeRevision++;
    selectedWorkspaceKey.value = key;
    if (key) expandWorkspace(key);
    selectedAgent.value = selectedWorkspace.value?.agents.length === 1 ? selectedWorkspace.value.agents[0]! : null;
    queryHistory();
    void refreshDriverResources();
  }
  function selectAgent(agent: string | null, key = selectedWorkspaceKey.value) {
    if (key !== selectedWorkspaceKey.value) selectWorkspace(key);
    routeRevision++;
    selectedAgent.value = agent;
    autoSelectSoleAgent = agent !== null;
    queryHistory();
    void refreshDriverResources();
  }
  function isWorkspaceExpanded(key: string) {
    return expandedWorkspaceKeys.value.has(key);
  }
  function toggleWorkspace(key: string) {
    const wasExpanded = expandedWorkspaceKeys.value.has(key);
    if (key !== selectedWorkspaceKey.value) selectWorkspace(key);
    const next = new Set(expandedWorkspaceKeys.value);
    if (wasExpanded) next.delete(key); else next.add(key);
    expandedWorkspaceKeys.value = next;
  }
  function expandWorkspace(key: string) {
    if (expandedWorkspaceKeys.value.has(key)) return;
    const next = new Set(expandedWorkspaceKeys.value);
    next.add(key);
    expandedWorkspaceKeys.value = next;
  }
  function executeMclCommand(
    command: string,
    argumentsValue: unknown[][] = [],
    id = crypto.randomUUID(),
  ): Promise<unknown> {
    const workspace = selectedWorkspace.value;
    if (!workspace || !command.trim())
      return Promise.reject(new Error('Select a workspace and enter an MCL command'));
    if (!selectedAgentReady.value)
      return Promise.reject(new Error('The selected agent is not running'));
    if (!Array.isArray(argumentsValue) || !argumentsValue.every(Array.isArray))
      return Promise.reject(new Error('Arguments must be arrays'));
    if (!socket || socket.readyState !== WebSocket.OPEN || !registeredClient.value)
      return Promise.reject(new Error('Register a daemon connection first'));
    const request: ClientMessage = {
      type: 'mcl.command',
      id,
      message: {
        workspace: { id: workspace.id, name: workspace.name, project_root: workspace.project_root },
        agent: selectedAgentId.value,
        command: command.trim(),
        arguments: argumentsValue,
      },
    };
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        pending.delete(id);
        reject(new Error('MCL result timed out; the backend command may still be running'));
      }, 120000);
      if (command.trim().startsWith('GET ')) readCommands.add(id);
      const failureKey = `${workspace.id}:${selectedAgentId.value}`;
      pending.set(id, {
        resolve,
        reject: (error) => {
          if (!readCommands.has(id)) lastFailures.value.set(failureKey, { message: error.message });
          reject(error);
        },
        timeout,
      });
      try {
        socket!.send(JSON.stringify(request));
      } catch (error) {
        clearTimeout(timeout);
        pending.delete(id);
        reject(error instanceof Error ? error : new Error(String(error)));
      }
    });
  }
  async function sendMessage(
    content: string,
    images: UserImage[] = [],
    onStage?: (stage: 'injecting' | 'waking') => void,
  ): Promise<unknown> {
    if (!content.trim() && !images.length) throw new Error('Message is empty');
    if (!selectedAgentIdle.value) throw new Error('The selected agent is not idle');
    const key = driverKey.value;
    const revision = routeRevision;
    const instruction = contextInjection(content.trim(), driverContextPath.value, images);
    lastFailures.value.delete(`${selectedWorkspaceKey.value}:${selectedAgentId.value}`);
    let stage: 'injecting' | 'waking' = 'injecting';
    onStage?.(stage);
    addLog('INFO', 'Sending message; waiting for daemon injection acknowledgement', 'ui.send', [
      { name: 'images', value: String(images.length) },
    ]);
    try {
      await executeMclCommand(instruction.command, instruction.arguments);
      if (revision !== routeRevision) throw new Error('Selected agent changed during send');
      assertDriverRoute(key);
      stage = 'waking';
      onStage?.(stage);
      addLog('INFO', 'Message injected; waiting for daemon wake acknowledgement', 'ui.send');
      const result = await executeMclCommand('EMIT EFFECT wake');
      addLog('INFO', 'Daemon acknowledged message and wake', 'ui.send');
      return result;
    } catch (error) {
      // Backend errors may contain submitted arguments; keep activity diagnostics payload-free.
      addLog('ERROR', `Message send failed while awaiting ${stage === 'injecting' ? 'injection' : 'wake'} acknowledgement; see composer error`, 'ui.send');
      throw error;
    }
  }
  function importCandidates() {
    const candidates = new Map<string, { resource_id: string; alias: string; kind: string }>();
    for (const row of selectedHistory.value?.messages ?? []) {
      const text =
        isRecord(row.payload) && typeof row.payload.text === 'string'
          ? row.payload.text
          : row.content;
      const match = /^IMPORT (MESSAGE|TOOL|SANDBOX) (\S+) AS ([A-Za-z_][A-Za-z0-9_-]*)$/.exec(
        text.trim(),
      );
      if (match)
        candidates.set(match[3]!, {
          resource_id: match[2]!,
          alias: match[3]!,
          kind: match[1] === 'TOOL' ? 'Tool' : match[1] === 'SANDBOX' ? 'Sandbox' : 'Message',
        });
    }
    return [...candidates.values()];
  }
  const refreshing = new Set<string>();
  const refreshedImports = new Map<string, string>();
  function assertDriverRoute(key: string) {
    if (driverKey.value !== key) throw new Error('Selected agent changed during the operation');
  }
  async function refreshDriverResources(force = false): Promise<void> {
    const key = driverKey.value;
    if (
      !key ||
      !selectedAgentReady.value ||
      !socket ||
      !registeredClient.value ||
      refreshing.has(key)
    )
      return;
    const candidates = importCandidates();
    const signature = JSON.stringify(candidates);
    if (!force && refreshedImports.get(key) === signature) return;
    refreshing.add(key);
    const snapshot = {
      imports: [] as ReturnType<typeof importCandidates>,
      visible: [] as Array<{ resource_id: string; alias: string }>,
      activated: [] as string[],
    };
    try {
      const visible = await executeMclCommand('GET session.visible');
      if (isRecord(visible) && isRecord(visible.Array) && Array.isArray(visible.Array.Tool))
        snapshot.visible = visible.Array.Tool.filter(
          (item): item is { resource_id: string; alias: string } =>
            isRecord(item) &&
            typeof item.resource_id === 'string' &&
            typeof item.alias === 'string',
        );
      for (const candidate of candidates) {
        assertDriverRoute(key);
        try {
          const result = await executeMclCommand(`GET ${candidate.alias}`);
          if (!isRecord(result) || !isRecord(result.Array)) continue;
          const values = result.Array[candidate.kind];
          if (
            Array.isArray(values) &&
            values.some((item) => isRecord(item) && item.resource_id === candidate.resource_id)
          )
            snapshot.imports.push(candidate);
        } catch {}
      }
      assertDriverRoute(key);
      const sandbox = await executeMclCommand('GET session.sandbox_activated');
      if (isRecord(sandbox) && isRecord(sandbox.Array) && Array.isArray(sandbox.Array.Sandbox))
        snapshot.activated = sandbox.Array.Sandbox.flatMap((item) =>
          isRecord(item) && typeof item.resource_id === 'string' ? [item.resource_id] : [],
        );
      assertDriverRoute(key);
      driverResources.value.set(key, snapshot);
      refreshedImports.set(key, signature);
    } catch (error) {
      addLog(
        'WARN',
        `Driver resource controls unavailable: ${errorMessage(error instanceof Error ? error.message : error)}`,
      );
    } finally {
      refreshing.delete(key);
    }
  }
  async function setResourceVisibility(resource: string, visible: boolean): Promise<unknown> {
    const key = driverKey.value;
    const state = driverState.value;
    const imported = state?.imports.find(
      (item) => item.resource_id === resource && item.kind === 'Tool',
    );
    const current = state?.visible.find((item) => item.resource_id === resource);
    if (!state || (!imported && !current))
      throw new Error('Resource has no verified current TOOL alias');
    const aliases = state.visible
      .filter((item) => item.resource_id !== resource)
      .map((item) => item.alias);
    if (visible) aliases.push((imported ?? current)!.alias);
    const result = await executeMclCommand('INJECT ? TO session.visible', [[...new Set(aliases)]]);
    assertDriverRoute(key);
    await refreshDriverResources(true);
    return result;
  }
  async function setSandbox(resource: string | null): Promise<unknown> {
    const key = driverKey.value;
    const imported = driverState.value?.imports.find(
      (item) => item.resource_id === resource && item.kind === 'Sandbox',
    );
    if (resource && !imported) throw new Error('Sandbox has no verified current SANDBOX alias');
    await executeMclCommand('INJECT ? TO session.sandbox_activated', [
      imported ? [imported.alias] : [],
    ]);
    assertDriverRoute(key);
    const result = await executeMclCommand('EMIT EFFECT sandbox_use (session.sandbox_activated)');
    assertDriverRoute(key);
    await refreshDriverResources(true);
    return result;
  }
  function abortTurn(): Promise<unknown> {
    const workspace = selectedWorkspace.value;
    const state = selectedAgentState.value;
    if (!workspace || !state || !canAbortTurn.value || !state.turn)
      return Promise.reject(new Error('No abortable turn is active'));
    if (!socket || socket.readyState !== WebSocket.OPEN || !registeredClient.value)
      return Promise.reject(new Error('Register a daemon connection first'));
    const id = crypto.randomUUID();
    const request: ClientMessage = {
      type: 'agent.turn.abort',
      id,
      message: {
        workspace: { id: workspace.id, name: workspace.name, project_root: workspace.project_root },
        agent: state.agent,
        turn_id: state.turn,
      },
    };
    abortRequests.value.set(id, {
      workspace: workspace.id,
      agent: state.agent,
      turn: state.turn,
      instance: state.agent_instance,
      streamKeys: [...streams.value].filter(([, stream]) =>
        stream.workspaceKey === workspace.id && stream.agent === state.agent &&
        stream.instance === state.agent_instance && !stream.completed && !abortedStreams.has(stream.key),
      ).map(([key]) => key),
    });
    const guard = abortRequests.value.get(id)!;
    abortGuards.set(id, guard);
    for (const key of guard.streamKeys) rememberAbortedStream(key);
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        pending.delete(id);
        abortRequests.value.delete(id);
        reject(new Error('Abort result timed out; cleanup may still be pending'));
      }, 120000);
      pending.set(id, { resolve, reject, timeout });
      try {
        socket!.send(JSON.stringify(request));
      } catch (error) {
        clearTimeout(timeout);
        pending.delete(id);
        releaseAbortGuard(id);
        abortRequests.value.delete(id);
        reject(error instanceof Error ? error : new Error(String(error)));
      }
    });
  }
  function rememberAbortedStream(key: string) {
    abortedStreams.add(key);
    while (abortedStreams.size > 1000)
      abortedStreams.delete(abortedStreams.values().next().value!);
  }
  function releaseAbortGuard(id: string) {
    const guard = abortGuards.get(id);
    if (!guard) return;
    // A late rejection must not revive old requests after an authoritative new turn.
    if (agentStates.value.some((state) =>
      state.workspace.id === guard.workspace && state.agent === guard.agent &&
      state.agent_instance === guard.instance && (state.turn === guard.turn || state.turn === null),
    )) {
      for (const key of guard.streamKeys) abortedStreams.delete(key);
    }
    abortGuards.delete(id);
  }
  function handleDelta(event: StreamDelta, reasoning: boolean) {
    const state = agentStates.value.find(
      (s) => s.agent === event.agent && s.agent_instance === event.agent_instance,
    );
    if (!state) return;
    const key = `${event.agent_instance}:${event.id}`;
    if (completedStreams.has(key) || abortedStreams.has(key)) return;
    const guard = [...abortGuards.values()].find((request) =>
      request.workspace === state.workspace.id && request.agent === event.agent &&
      request.instance === event.agent_instance &&
      (state.turn === request.turn || state.turn === null),
    );
    if (guard) {
      guard.streamKeys.push(key);
      rememberAbortedStream(key);
      return;
    }
    const old = streams.value.get(key);
    if (old && (old.completed || event.sequence <= old.lastSequence)) return;
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
          instance: event.agent_instance,
          lastSequence: -1,
          completed: false,
        };
    if (reasoning) message.thinking += event.content;
    else message.content += event.content;
    message.lastSequence = event.sequence;
    streams.value = new Map(streams.value).set(key, message);
  }
  function handleServerMessage(event: ServerMessage) {
    switch (event.type) {
      case 'connection.registered':
        registeredClient.value = event.client;
        connectionStatus.value = 'online';
        connectionError.value = '';
        reconnectAttempt = 0;
        socket?.send(
          JSON.stringify({
            type: 'state.query',
            id: crypto.randomUUID(),
            message: {},
          } satisfies ClientMessage),
        );
        socket?.send(JSON.stringify({ type: 'workspace.catalog.query', id: crypto.randomUUID(), message: {} } satisfies ClientMessage));
        break;
      case 'connection.register_failed':
        connectionError.value = errorMessage(event.error);
        shouldReconnect = false;
        socket?.close();
        break;
      case 'state.sync': {
        runtimeWorkspaces.value = event.state.workspaces;
        catalog.value = event.state.catalog ?? [];
        if (Array.isArray(event.state.drivers)) advertisedDrivers.value = event.state.drivers;
        agentStates.value = event.state.agents;
        for (const [id, guard] of abortGuards) {
          if (!agentStates.value.some((state) =>
            state.workspace.id === guard.workspace && state.agent === guard.agent &&
            state.agent_instance === guard.instance &&
            (state.turn === guard.turn || state.turn === null),
          )) abortGuards.delete(id);
        }
        for (const [id, request] of abortRequests.value) {
          if (
            !pending.has(id) &&
            !agentStates.value.some(
              (state) =>
                state.workspace.id === request.workspace &&
                state.agent === request.agent &&
                state.agent_instance === request.instance &&
                state.status === 'working' &&
                (state.turn === request.turn || state.turn === null),
            )
          )
            abortRequests.value.delete(id);
        }
        for (const state of agentStates.value) {
          if (!state.error && ['idle', 'working'].includes(state.status)) lastFailures.value.delete(`${state.workspace.id}:${state.agent}`);
          if (state.error) {
            lastFailures.value.set(`${state.workspace.id}:${state.agent}`, {
              message: errorMessage(state.error),
            });
          }
        }
        histories.value = event.state.histories;
        reconcileSelection();
        void refreshDriverResources();
        const allHistory = histories.value.flatMap(historyEntries);
        streams.value = new Map(
          [...streams.value].filter(
            ([, stream]) =>
              agentStates.value.some(
                (s) =>
                  s.agent === stream.agent &&
                  s.workspace.id === stream.workspaceKey &&
                  s.agent_instance === stream.instance,
              ) && !allHistory.some((h) => h.id === stream.id && h.role === 'assistant'),
          ),
        );
        break;
      }
      case 'driver.catalog':
        queriedDrivers.value = event.drivers;
        driverCatalogErrors.value = event.errors;
        settleOperation(event.id, event);
        break;
      case 'workspace.paths':
        settleOperation(event.id, event);
        break;
      case 'workspace.catalog':
        catalog.value = event.entries;
        reconcileSelection();
        settleOperation(event.id, event);
        break;
      case 'workspace.member.result':
        if (!pending.has(event.id)) break;
        if (event.entry) catalog.value = [...catalog.value.filter((entry) => entry.id !== event.entry!.id), event.entry];
        settleOperation(event.id, event.entry, event.error ? errorMessage(event.error) : undefined);
        break;
      case 'workspace.started':
        settleOperation(event.id, event.workspace);
        queryCatalog();
        for (const failure of event.workspace.member_failures ?? []) {
          lastFailures.value.set(`${event.workspace.id}:${failure.agent}`, { message: failure.error });
          addLog('ERROR', `Agent ${failure.agent} (${failure.driver}) failed to start: ${failure.error}`);
        }
        runtimeWorkspaces.value = [
          ...runtimeWorkspaces.value.filter((w) => w.id !== event.workspace.id),
          event.workspace,
        ];
        if (!selectedWorkspaceKey.value) selectWorkspace(event.workspace.id);
        break;
      case 'workspace.stopped':
        settleOperation(event.id, event.workspace);
        queryCatalog();
        runtimeWorkspaces.value = runtimeWorkspaces.value.filter((w) => w.id !== event.workspace.id);
        break;
      case 'workspace.start_failed':
      case 'workspace.stop_failed':
        settleOperation(event.id, null, errorMessage(event.error));
        addLog('ERROR', errorMessage(event.error));
        break;
      case 'agent.turn.abort_result': {
        const request = abortRequests.value.get(event.id);
        if (
          !request ||
          request.workspace !== event.workspace.id ||
          request.agent !== event.agent ||
          request.turn !== event.turn_id
        )
          break;
        if ('Err' in event.result) {
          releaseAbortGuard(event.id);
          abortRequests.value.delete(event.id);
        }
        const command = pending.get(event.id);
        if (command) {
          clearTimeout(command.timeout);
          pending.delete(event.id);
          if ('Ok' in event.result) command.resolve(null);
          else {
            abortRequests.value.delete(event.id);
            command.reject(new Error(errorMessage(event.result.Err)));
          }
        }
        break;
      }
      case 'mcl.command_result': {
        const command = pending.get(event.id);
        if (command) {
          clearTimeout(command.timeout);
          pending.delete(event.id);
          if ('Ok' in event.result) command.resolve(event.result.Ok);
          else command.reject(new Error(errorMessage(event.result.Err)));
        }
        const wasRead = readCommands.delete(event.id);
        if (!wasRead) queryHistory();
        if (event.history_error)
          addLog(
            'WARN',
            `Command completed but history persistence failed: ${errorMessage(event.history_error)}`,
          );
        break;
      }
      case 'agent.message.delta':
        handleDelta(event, false);
        break;
      case 'agent.message.reasoning_delta':
        handleDelta(event, true);
        break;
      case 'history.result':
        histories.value = [
          ...histories.value.filter(
            (h) => h.agent !== event.history.agent || h.workspace.id !== event.history.workspace.id,
          ),
          event.history,
        ];
        void refreshDriverResources();
        streams.value = new Map(
          [...streams.value].filter(
            ([, stream]) =>
              !stream.completed ||
              stream.agent !== event.history.agent ||
              stream.workspaceKey !== event.history.workspace.id,
          ),
        );
        break;
      case 'request.failed': {
        releaseAbortGuard(event.id);
        abortRequests.value.delete(event.id);
        const command = pending.get(event.id);
        if (command) {
          clearTimeout(command.timeout);
          pending.delete(event.id);
          command.reject(new Error(errorMessage(event.error)));
        } else addLog('ERROR', errorMessage(event.error));
        break;
      }
      case 'agent.message.finished': {
        if (
          !agentStates.value.some(
            (state) => state.agent === event.agent && state.agent_instance === event.agent_instance,
          )
        )
          break;
        const key = `${event.agent_instance}:${event.id}`;
        if (completedStreams.has(key)) break;
        completedStreams.add(key);
        if (completedStreams.size > 1000)
          completedStreams.delete(completedStreams.values().next().value!);
        const old = streams.value.get(key);
        if (old)
          streams.value = new Map(streams.value).set(key, {
            ...old,
            completed: true,
            failed: !!event.error,
          });
        if (event.error) {
          const state = agentStates.value.find(
            (item) => item.agent === event.agent && item.agent_instance === event.agent_instance,
          )!;
          lastFailures.value.set(`${state.workspace.id}:${event.agent}`, {
            message: errorMessage(event.error),
          });
          addLog('ERROR', errorMessage(event.error), event.agent);
        }
        queryHistory();
        break;
      }
      case 'log':
        logs.value.push({
          key: crypto.randomUUID(),
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
  function reconcileSelection() {
    const validKeys = new Set(workspaces.value.map((workspace) => workspace.id));
    expandedWorkspaceKeys.value = new Set([...expandedWorkspaceKeys.value].filter((key) => validKeys.has(key) || renamingWorkspaces.has(key)));
    if (!selectionInitialized && workspaces.value.length) selectWorkspace(workspaces.value[0]!.id);
    else if (selectedWorkspaceKey.value && !validKeys.has(selectedWorkspaceKey.value) && !renamingWorkspaces.has(selectedWorkspaceKey.value)) selectWorkspace('');
    if (selectedAgent.value && !selectedWorkspace.value?.agents.includes(selectedAgent.value) && !renamingWorkspaces.has(selectedWorkspaceKey.value) && !renamingMembers.has(`${selectedWorkspaceKey.value}:${selectedAgent.value}`)) selectAgent(null);
    if (autoSelectSoleAgent && !selectedAgent.value && selectedWorkspace.value?.agents.length === 1) selectAgent(selectedWorkspace.value.agents[0]!);
  }
  function settleOperation(id: string, result: unknown, error?: string) {
    const operation = pending.get(id);
    if (!operation) return;
    clearTimeout(operation.timeout);
    pending.delete(id);
    if (error) operation.reject(new Error(error)); else operation.resolve(result);
  }
  async function workspaceRequest(request: ClientMessage) {
    if (!socket || socket.readyState !== WebSocket.OPEN || !registeredClient.value) throw new Error('Connect to the daemon first');
    workspaceOperationPending.value = true;
    workspaceNotice.value = '';
    try {
      const result = await new Promise((resolve, reject) => {
        const timeout = setTimeout(() => { pending.delete(request.id); reject(new Error('Request timed out; refresh before retrying')); }, 30000);
        pending.set(request.id, { resolve, reject, timeout });
        socket!.send(JSON.stringify(request));
      });
      if (['workspace.create', 'workspace.rename', 'workspace.delete', 'workspace.member.add', 'workspace.member.remove', 'workspace.member.rename'].includes(request.type)) queryCatalog();
      return result;
    } catch (error) {
      workspaceNotice.value = error instanceof Error ? error.message : String(error);
      socket?.send(JSON.stringify({ type: 'workspace.catalog.query', id: crypto.randomUUID(), message: {} } satisfies ClientMessage));
      throw error;
    } finally { workspaceOperationPending.value = false; }
  }
  function queryCatalog() {
    socket?.send(JSON.stringify({ type: 'workspace.catalog.query', id: crypto.randomUUID(), message: {} } satisfies ClientMessage));
  }
  function refreshCatalog(refresh = true) {
    return workspaceRequest({ type: refresh ? 'workspace.catalog.refresh' : 'workspace.catalog.query', id: crypto.randomUUID(), message: {} });
  }
  async function startWorkspace() {
    if (workspaceOperationPending.value) throw new Error('Wait for the current workspace operation');
    const entry = selectedCatalogEntry.value;
    if (!entry?.definition || entry.error) throw new Error('Fix the registered workspace definition and refresh first');
    await workspaceRequest({ type: 'workspace.start', id: crypto.randomUUID(), message: { workspace: entry.id, expected_revision: entry.revision } });
    workspaceNotice.value = 'Workspace started.';
  }
  async function stopWorkspace() {
    if (workspaceOperationPending.value) throw new Error('Wait for the current workspace operation');
    const workspace = selectedWorkspace.value;
    if (!workspace) return;
    await workspaceRequest({ type: 'workspace.stop', id: crypto.randomUUID(), message: { workspace } });
    workspaceNotice.value = 'Workspace stopped.';
  }
  function targetEntry(target?: { key: string; revision: string }) {
    const entry = catalogEntry(target?.key ?? selectedWorkspaceKey.value);
    return { ...entry, revision: target?.revision ?? entry.revision };
  }
  async function queryDrivers(refresh = false) {
    return await workspaceRequest({ type: refresh ? 'driver.catalog.refresh' : 'driver.catalog.query', id: crypto.randomUUID(), message: {} }) as DriverCatalog;
  }
  async function queryPaths(message: { path?: string; cursor?: string } = {}) {
    return await workspaceRequest({ type: 'workspace.paths.query', id: crypto.randomUUID(), message }) as WorkspacePaths;
  }
  async function addMember(name: string, driver: string, target?: { key: string; revision: string }) {
    if (workspaceOperationPending.value) throw new Error('Wait for the current workspace operation');
    const entry = targetEntry(target);
    name = validName(name, 'Agent');
    if (!drivers.value.includes(driver)) throw new Error('Select a validated installed driver');
    const result = await workspaceRequest({ type: 'workspace.member.add', id: crypto.randomUUID(), message: { workspace: entry.id, expected_revision: entry.revision, agent: { name, driver } } });
    reconcileSelection();
    workspaceNotice.value = 'Agent saved.';
    return result;
  }
  async function removeMember(agent: string, target?: { key: string; revision: string }) {
    if (workspaceOperationPending.value) throw new Error('Wait for the current workspace operation');
    const entry = targetEntry(target);
    const result = await workspaceRequest({ type: 'workspace.member.remove', id: crypto.randomUUID(), message: { workspace: entry.id, expected_revision: entry.revision, agent } });
    if (selectedWorkspaceKey.value === entry.id && selectedAgent.value === agent) selectAgent(null);
    workspaceNotice.value = 'Agent removed. History retained.';
    return result;
  }
  /**
   * Sends one catalog mutation. The catalog is never mutated optimistically:
   * every response reuses `workspace.member.result` with the authoritative
   * entry (or a structured error that is surfaced verbatim).
   */
  async function catalogRequest<T extends Extract<ClientMessage, { type: CatalogRequestType }>>(
    request: Omit<T, 'id'>,
  ) {
    if (!SUPPORTED_CATALOG_REQUESTS[request.type])
      throw new Error(`The daemon does not support ${request.type} yet; no definition was changed`);
    const wire = { ...request, id: crypto.randomUUID() } as unknown as ClientMessage;
    const workspace = wire.type === 'workspace.rename' ? wire.message.workspace : null;
    const member = wire.type === 'workspace.member.rename' ? `${wire.message.workspace}:${wire.message.agent}` : null;
    if (workspace) renamingWorkspaces.add(workspace);
    if (member) renamingMembers.add(member);
    try { return await workspaceRequest(wire); }
    finally {
      if (workspace) renamingWorkspaces.delete(workspace);
      if (member) renamingMembers.delete(member);
    }
  }
  function catalogEntry(key: string) {
    const entry = catalog.value.find((item) => item.id === key);
    if (!entry) throw new Error('Workspace is not registered');
    return entry;
  }
  function validName(name: string, label: string) {
    const value = name.trim();
    if (!value) throw new Error(`${label} name is required`);
    if (
      value === '.' ||
      value === '..' ||
      value.length > 128 ||
      value.includes('/') ||
      value.includes('\\') ||
      [...value].some((character) => character.charCodeAt(0) < 32)
    )
      throw new Error(`${label} name cannot contain slashes or control characters (max 128)`);
    return value;
  }
  async function createWorkspace(input: { name: string; projectRoot: string }) {
    if (workspaceOperationPending.value) throw new Error('Wait for the current workspace operation');
    const name = validName(input.name, 'Workspace');
    const projectRoot = input.projectRoot.trim();
    if (!projectRoot.startsWith('/'))
      throw new Error('Project path must be an absolute path starting with "/"');
    const entry = await catalogRequest({ type: CATALOG_REQUESTS.createWorkspace, message: { name, project_root: projectRoot } });
    workspaceNotice.value = 'Workspace created.';
    if (isRecord(entry) && typeof entry.id === 'string') {
      expandWorkspace(entry.id);
      selectWorkspace(entry.id);
    }
    return entry;
  }
  async function renameWorkspace(key: string, name: string, revision?: string) {
    if (workspaceOperationPending.value) throw new Error('Wait for the current workspace operation');
    const entry = catalogEntry(key);
    const newName = validName(name, 'Workspace');
    if (newName === entry.name) return entry;
    const result = await catalogRequest({
      type: CATALOG_REQUESTS.renameWorkspace,
      message: { workspace: entry.id, expected_revision: revision ?? entry.revision, new_name: newName },
    });
    if (isRecord(result) && typeof result.id === 'string') {
      catalog.value = catalog.value.filter((item) => item.id !== key || item.id === result.id);
      const wasExpanded = expandedWorkspaceKeys.value.delete(key);
      if (wasExpanded) expandWorkspace(result.id);
      if (selectedWorkspaceKey.value === key) {
        const agentName = entry.definition?.agents.find((agent) => agent.id === selectedAgent.value)?.name;
        selectWorkspace(result.id);
        const renamed = (result as unknown as CatalogEntry).definition?.agents.find((agent) => agent.name === agentName);
        if (renamed) selectAgent(renamed.id);
      }
    }
    workspaceNotice.value = 'Workspace renamed.';
    return result;
  }
  async function deleteWorkspace(key: string, revision?: string) {
    if (workspaceOperationPending.value) throw new Error('Wait for the current workspace operation');
    const entry = catalogEntry(key);
    const result = await catalogRequest({
      type: CATALOG_REQUESTS.deleteWorkspace,
      message: { workspace: entry.id, expected_revision: revision ?? entry.revision },
    });
    catalog.value = catalog.value.filter((item) => item.id !== entry.id);
    if (selectedWorkspaceKey.value === entry.id) selectWorkspace('');
    const next = new Set(expandedWorkspaceKeys.value);
    next.delete(entry.id);
    expandedWorkspaceKeys.value = next;
    workspaceNotice.value = 'Workspace moved to trash.';
    return result;
  }
  /**
   * Renaming never stops the runtime locally. A daemon that refuses while the
   * agent is running returns an error that the dialog shows as-is.
   */
  async function renameMember(agent: string, name: string, target?: { key: string; revision: string }) {
    if (workspaceOperationPending.value) throw new Error('Wait for the current workspace operation');
    const entry = targetEntry(target);
    const newName = validName(name, 'Agent');
    const current = entry.definition?.agents.find((member) => member.id === agent);
    if (current && newName === current.name) return current;
    const result = await catalogRequest({
      type: CATALOG_REQUESTS.renameMember,
      message: { workspace: entry.id, expected_revision: entry.revision, agent, new_name: newName },
    });
    if (selectedWorkspaceKey.value === entry.id && selectedAgent.value === agent && isRecord(result)) {
      const renamed = (result as unknown as CatalogEntry).definition?.agents.find((member) => member.name === newName);
      selectAgent(renamed?.id ?? null);
    }
    lastFailures.value.delete(`${entry.id}:${agent}`);
    workspaceNotice.value = 'Agent renamed.';
    return result;
  }
  async function deleteMember(agent: string, target?: { key: string; revision: string }) {
    return removeMember(agent, target);
  }
  function newSessionDefaults() {
    return {
      drivers: drivers.value,
      workspace: selectedCatalogEntry.value,
      usable: !!selectedCatalogEntry.value?.definition && !selectedCatalogEntry.value?.error,
    };
  }
  function clearLogs() {
    logs.value = [];
  }
  return {
    endpoint,
    driverContextPath,
    connectionStatus,
    connectionError,
    registeredClient,
    workspaces,
    catalog,
    selectedCatalogEntry,
    workspaceOperationPending,
    workspaceNotice,
    refreshCatalog,
    startWorkspace,
    stopWorkspace,
    addMember,
    removeMember,
    deleteMember,
    drivers,
    advertisedDrivers,
    driverCatalogErrors,
    queryDrivers,
    queryPaths,
    sidebarTree,
    isWorkspaceExpanded,
    toggleWorkspace,
    expandWorkspace,
    createWorkspace,
    renameWorkspace,
    deleteWorkspace,
    renameMember,
    newSessionDefaults,
    agentStates,
    selectedWorkspaceKey,
    selectedWorkspace,
    selectedAgent,
    selectedAgentName,
    selectedAgentState,
    selectedAgentReady,
    selectedAgentIdle,
    abortPending,
    selectedAgentWorking,
    loadedResources,
    resourceVisibility,
    sandboxOptions,
    lastFailures,
    canAbortTurn,
    refreshDriverResources,
    setResourceVisibility,
    setSandbox,
    abortTurn,
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
    clearLogs,
    handleServerMessage,
  };
});
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
