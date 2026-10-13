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
  if (!path.trim() || !/^[A-Za-z_][A-Za-z0-9_.\[\]-]*$/.test(path.trim()))
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
    const payload =
      isRecord(entry.payload) && isAgentMessage(entry.payload.message)
        ? entry.payload.message
        : entry.payload;
    if (isAgentMessage(payload))
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
function payloadRequestId(entry: HistoryMessage): string {
  return isRecord(entry.payload) && typeof entry.payload.id === 'string'
    ? entry.payload.id
    : entry.key;
}
function decodeMessage(
  message: AgentMessage,
): Array<Pick<ConversationEntry, 'role' | 'thinking' | 'content' | 'toolCalls' | 'failed'>> {
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
  const driverContextPath = ref('');
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
  const selectedAgentReady = computed(() => selectedAgentState.value?.status === 'running');
  const selectedAgentWorking = computed(() => !!selectedAgentState.value?.pending_command);
  const loadedResources = computed(() => selectedAgentState.value?.resources ?? []);
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
  }
  function selectAgent(agent: string | null) {
    selectedAgent.value = agent;
    queryHistory();
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
      pending.set(id, { resolve, reject, timeout });
      try {
        socket!.send(JSON.stringify(request));
      } catch (error) {
        clearTimeout(timeout);
        pending.delete(id);
        reject(error instanceof Error ? error : new Error(String(error)));
      }
    });
  }
  function sendMessage(content: string): Promise<unknown> {
    if (!content.trim()) return Promise.reject(new Error('Message is empty'));
    try {
      const instruction = contextInjection(content.trim(), driverContextPath.value);
      return executeMclCommand(instruction.command, instruction.arguments);
    } catch (error) {
      return Promise.reject(error);
    }
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
        histories.value = event.state.histories;
        if (!workspaces.value.some((w) => w.id === selectedWorkspaceKey.value))
          selectWorkspace(workspaces.value[0]?.id ?? '');
        if (selectedAgent.value && !selectedWorkspace.value?.agents.includes(selectedAgent.value))
          selectedAgent.value = null;
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
      case 'mcl.command_result': {
        const command = pending.get(event.id);
        if (command) {
          clearTimeout(command.timeout);
          pending.delete(event.id);
          if ('Ok' in event.result) command.resolve(event.result.Ok);
          else command.reject(new Error(errorMessage(event.result.Err)));
        }
        queryHistory();
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
        if (event.error) addLog('ERROR', errorMessage(event.error), event.agent);
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
    selectedAgentWorking,
    loadedResources,
    visibleMessages,
    tokenUsage,
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
