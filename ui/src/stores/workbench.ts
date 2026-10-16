import { computed, ref } from 'vue';
import { defineStore } from 'pinia';
import {
  parseServerMessage,
  isRecord,
  isAgentMessage,
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
  type WorkspaceInfo,
  type WorkspaceRef,
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
  aborted?: boolean;
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
): { command: string; arguments: unknown[][] } {
  if (!path.trim() || !/^[A-Za-z_][A-Za-z0-9_.[\]-]*$/.test(path.trim()))
    throw new Error('Configure a valid driver context path before injecting a message');
  return { command: `INJECT ? TO ${path.trim()}`, arguments: [[{ type: 'user', content }]] };
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
  Pick<ConversationEntry, 'role' | 'thinking' | 'content' | 'toolCalls' | 'failed' | 'aborted'>
> {
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
        },
      ];
    case 'assistant':
      return [
        {
          role: 'assistant',
          thinking: message.reasoning ?? '',
          content: message.stop_reason === 'cancelled' ? 'Turn aborted.' : (message.content ?? ''),
          aborted: message.stop_reason === 'cancelled',
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
    new Map<string, { workspace: string; agent: string; turn: string; instance: string }>(),
  );
  const connectionStatus = ref<ConnectionStatus>('offline');
  const connectionError = ref('');
  const registeredClient = ref<ClientInfo | null>(null);
  const workspaces = ref<WorkspaceInfo[]>([]);
  const agentStates = ref<AgentState[]>([]);
  const histories = ref<AgentHistory[]>([]);
  const selectedWorkspaceKey = ref('');
  const selectedAgent = ref<string | null>(null);
  const logs = ref<RuntimeLog[]>([]);
  const streams = ref(new Map<string, StreamEntry>());
  const pending = new Map<string, PendingCommand>();
  const completedStreams = new Set<string>();
  const readCommands = new Set<string>();
  let socket: WebSocket | null = null;
  let reconnectTimer: ReturnType<typeof setTimeout> | undefined;
  let reconnectAttempt = 0;
  let shouldReconnect = true;
  const selectedWorkspace = computed(
    () => workspaces.value.find((w) => workspaceKey(w) === selectedWorkspaceKey.value) ?? null,
  );
  const selectedAgentId = computed(
    () => selectedAgent.value || selectedWorkspace.value?.manager || '',
  );
  const selectedAgentName = computed(() => resourceName(selectedAgentId.value) || 'Manager');
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
    if (workspace && socket?.readyState === WebSocket.OPEN && registeredClient.value) {
      const request: ClientMessage = {
        type: 'history.query',
        id: crypto.randomUUID(),
        message: { workspace, agent: selectedAgent.value },
      };
      socket.send(JSON.stringify(request));
    }
  }
  function selectWorkspace(key: string) {
    selectedWorkspaceKey.value = key;
    selectedAgent.value = null;
    queryHistory();
    void refreshDriverResources();
  }
  function selectAgent(agent: string | null) {
    selectedAgent.value = agent;
    queryHistory();
    void refreshDriverResources();
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
        agent: selectedAgent.value,
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
  async function sendMessage(content: string): Promise<unknown> {
    if (!content.trim()) throw new Error('Message is empty');
    if (!selectedAgentIdle.value) throw new Error('The selected agent is not idle');
    const key = driverKey.value;
    const instruction = contextInjection(content.trim(), driverContextPath.value);
    lastFailures.value.delete(`${selectedWorkspaceKey.value}:${selectedAgentId.value}`);
    await executeMclCommand(instruction.command, instruction.arguments);
    assertDriverRoute(key);
    return executeMclCommand('EMIT EFFECT wake');
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
    });
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
        abortRequests.value.delete(id);
        reject(error instanceof Error ? error : new Error(String(error)));
      }
    });
  }
  function handleDelta(event: StreamDelta, reasoning: boolean) {
    const state = agentStates.value.find(
      (s) => s.agent === event.agent && s.agent_instance === event.agent_instance,
    );
    if (!state) return;
    const key = `${event.agent_instance}:${event.id}`;
    if (completedStreams.has(key)) return;
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
        break;
      case 'connection.register_failed':
        connectionError.value = errorMessage(event.error);
        shouldReconnect = false;
        socket?.close();
        break;
      case 'state.sync': {
        workspaces.value = event.state.workspaces;
        agentStates.value = event.state.agents;
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
          if (state.error) {
            lastFailures.value.set(`${state.workspace.id}:${state.agent}`, {
              message: errorMessage(state.error),
            });
          }
        }
        histories.value = event.state.histories;
        if (!workspaces.value.some((w) => w.id === selectedWorkspaceKey.value))
          selectWorkspace(workspaces.value[0]?.id ?? '');
        if (selectedAgent.value && !selectedWorkspace.value?.agents.includes(selectedAgent.value))
          selectedAgent.value = null;
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
      case 'workspace.started':
        workspaces.value = [
          ...workspaces.value.filter((w) => w.id !== event.workspace.id),
          event.workspace,
        ];
        if (!selectedWorkspaceKey.value) selectWorkspace(event.workspace.id);
        break;
      case 'workspace.stopped':
        workspaces.value = workspaces.value.filter((w) => w.id !== event.workspace.id);
        break;
      case 'workspace.start_failed':
      case 'workspace.stop_failed':
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
