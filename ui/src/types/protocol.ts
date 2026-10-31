export interface WorkspaceRef {
  id: string;
  name: string;
  project_root: string;
}
export type WorkspaceReference = WorkspaceRef | string;
export type WorkspaceStatus = 'starting' | 'running' | 'restarting' | 'stopping';
export interface WorkspaceInfo extends WorkspaceRef {
  definition_path: string;
  status: WorkspaceStatus;
  agents: string[];
  member_failures?: { agent: string; driver: string; error: string }[];
}
export interface AgentDefinition {
  name: string;
  driver: string;
  home_root: string;
}
export interface WorkspaceDefinition {
  name: string;
  project_root: string;
  agents: AgentDefinition[];
}
export interface WorkspaceConfig {
  path: string;
  cache: WorkspaceDefinition;
}
export interface ClientInfo {
  resource_id: string;
  client_type: string;
  name: string;
}
export interface ProtocolError {
  kind: string;
  message: string;
}
export interface TokenUsage {
  input_tokens: number;
  output_tokens: number;
  cache_hit_tokens?: number | null;
}
export interface ToolCall {
  id: string;
  tool_name: string;
  arguments: unknown;
}
export interface UserImage {
  media_type: string;
  data: string;
}
export type AgentMessage =
  | { type: 'system'; content: string }
  | { type: 'user'; content: string; images?: UserImage[] }
  | {
      type: 'assistant';
      reasoning: string | null;
      content: string | null;
      tool_calls: ToolCall[];
      usage?: TokenUsage | null;
      replay?: unknown;
      stop_reason?:
        | 'completed'
        | 'tool_calls'
        | 'length'
        | 'content_filter'
        | 'cancelled'
        | { other: string }
        | null;
    }
  | {
      type: 'tool_call';
      images?: UserImage[];
      resource_id?: string | null;
      tool_call_id: string;
      tool_name: string;
      content: string;
      failed: boolean;
    }
  | { type: 'error'; message: string }
  | { type: 'inject'; messages: AgentMessage[] };
export interface AgentState {
  workspace: WorkspaceRef;
  agent: string;
  agent_instance: string;
  status: 'starting' | 'idle' | 'working' | 'restarting' | 'stopping';
  turn: string | null;
  sandbox: string | null;
  queued_commands: number;
  pending_command: string | null;
  context_window_tokens?: number | null;
  error?: ProtocolError | null;
}
export interface HistoryMessage {
  key: string;
  sequence: number;
  turn: string | null;
  kind: string;
  content: string;
  payload: unknown;
  created_at_ms: number;
}
export interface AgentHistory {
  workspace: WorkspaceRef;
  agent: string;
  messages: HistoryMessage[];
}
export interface BackendState {
  workspace_definitions: WorkspaceConfig[];
  workspaces: WorkspaceInfo[];
  agents: AgentState[];
  histories: AgentHistory[];
}
export interface LogRecord {
  timestamp_millis: number;
  level: string;
  target: string;
  message: string;
  fields: { name: string; value: string }[];
  spans?: string[];
}
export interface StreamDelta {
  id: string;
  agent: string;
  agent_instance: string;
  sequence: number;
  content: string;
}
export type MclCommandResult = { Ok: unknown } | { Err: ProtocolError };
export type ClientMessage =
  | {
      type: 'connection.register';
      id: string;
      message: { client_type: string; name?: string | null };
    }
  | {
      type: 'state.query' | 'workspace.definitions.query';
      id: string;
      message: Record<string, never>;
    }
  | { type: 'workspace.start' | 'workspace.restart'; id: string; message: { path: string } }
  | { type: 'workspace.stop'; id: string; message: { workspace: WorkspaceReference } }
  | { type: 'history.query'; id: string; message: { workspace: WorkspaceReference; agent: string } }
  | {
      type: 'mcl.command';
      id: string;
      message: {
        workspace: WorkspaceReference;
        agent: string;
        command: string;
        arguments: unknown[];
      };
    }
  | {
      type: 'agent.turn.abort';
      id: string;
      message: {
        workspace: WorkspaceReference;
        agent: string;
        agent_instance: string;
        turn_id: string;
      };
    }
  | {
      type: 'agent.start' | 'agent.restart';
      id: string;
      message: {
        workspace: string;
        name: string;
        parameters: { driver: string; home_root: string };
      };
    };
export type ServerMessage =
  | { type: 'connection.registered'; id: string; client: ClientInfo }
  | { type: 'connection.register_failed'; id: string; error: ProtocolError }
  | { type: 'state.sync'; state: BackendState }
  | { type: 'workspace.definitions'; id: string; definitions: WorkspaceConfig[] }
  | { type: 'workspace.started' | 'workspace.restarted'; id: string; workspace: WorkspaceInfo }
  | { type: 'workspace.stopped'; id: string; workspace: WorkspaceReference }
  | {
      type:
        | 'workspace.start_failed'
        | 'workspace.restart_failed'
        | 'workspace.stop_failed'
        | 'request.failed';
      id: string;
      error: ProtocolError;
    }
  | { type: 'agent.started' | 'agent.restarted'; id: string; agent: string }
  | { type: 'history.result'; id: string; history: AgentHistory }
  | {
      type: 'mcl.command_result';
      id: string;
      source: string;
      target: string;
      result: MclCommandResult;
      history_error?: ProtocolError | null;
    }
  | {
      type: 'agent.turn.abort_result';
      id: string;
      workspace: WorkspaceReference;
      agent: string;
      turn_id: string;
      result: { Ok: null } | { Err: ProtocolError };
    }
  | ({ type: 'agent.message.delta' | 'agent.message.reasoning_delta' } & StreamDelta)
  | {
      type: 'agent.message.finished';
      id: string;
      agent: string;
      agent_instance: string;
      error: ProtocolError | null;
    }
  | { type: 'log'; record: LogRecord };

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}
export function errorMessage(value: unknown): string {
  return isRecord(value) && typeof value.message === 'string'
    ? value.message
    : typeof value === 'string'
      ? value
      : 'Unknown backend error';
}
const string = (value: unknown): value is string => typeof value === 'string';
const nullableString = (value: unknown): boolean => value === null || string(value);
const integer = (value: unknown): boolean =>
  typeof value === 'number' && Number.isSafeInteger(value) && value >= 0;
const strings = (value: unknown): value is string[] => Array.isArray(value) && value.every(string);
const protocolError = (value: unknown): boolean =>
  isRecord(value) && string(value.kind) && string(value.message);
const workspaceRef = (value: unknown): boolean =>
  isRecord(value) && string(value.id) && string(value.name) && string(value.project_root);
const workspaceReference = (value: unknown): boolean => string(value) || workspaceRef(value);
function workspaceConfig(value: unknown): boolean {
  return (
    isRecord(value) &&
    string(value.path) &&
    isRecord(value.cache) &&
    string(value.cache.name) &&
    string(value.cache.project_root) &&
    Array.isArray(value.cache.agents) &&
    value.cache.agents.every(
      (agent) =>
        isRecord(agent) && string(agent.name) && string(agent.driver) && string(agent.home_root),
    )
  );
}
function workspaceInfo(value: unknown): boolean {
  return (
    workspaceRef(value) &&
    isRecord(value) &&
    string(value.definition_path) &&
    ['starting', 'running', 'restarting', 'stopping'].includes(String(value.status)) &&
    strings(value.agents) &&
    (value.member_failures === undefined ||
      (Array.isArray(value.member_failures) &&
        value.member_failures.every(
          (failure) =>
            isRecord(failure) &&
            string(failure.agent) &&
            string(failure.driver) &&
            string(failure.error),
        )))
  );
}
function agentState(value: unknown): boolean {
  return (
    isRecord(value) &&
    workspaceRef(value.workspace) &&
    string(value.agent) &&
    string(value.agent_instance) &&
    ['starting', 'idle', 'working', 'restarting', 'stopping'].includes(String(value.status)) &&
    nullableString(value.turn) &&
    nullableString(value.sandbox) &&
    integer(value.queued_commands) &&
    nullableString(value.pending_command) &&
    (value.context_window_tokens == null || integer(value.context_window_tokens)) &&
    (value.error == null || protocolError(value.error))
  );
}
function history(value: unknown): boolean {
  return (
    isRecord(value) &&
    workspaceRef(value.workspace) &&
    string(value.agent) &&
    Array.isArray(value.messages) &&
    value.messages.every(
      (entry) =>
        isRecord(entry) &&
        string(entry.key) &&
        integer(entry.sequence) &&
        nullableString(entry.turn) &&
        string(entry.kind) &&
        string(entry.content) &&
        integer(entry.created_at_ms) &&
        'payload' in entry,
    )
  );
}
function backendState(value: unknown): boolean {
  return (
    isRecord(value) &&
    Array.isArray(value.workspace_definitions) &&
    value.workspace_definitions.every(workspaceConfig) &&
    Array.isArray(value.workspaces) &&
    value.workspaces.every(workspaceInfo) &&
    Array.isArray(value.agents) &&
    value.agents.every(agentState) &&
    Array.isArray(value.histories) &&
    value.histories.every(history)
  );
}
function result(value: unknown): boolean {
  return (
    isRecord(value) &&
    (('Ok' in value && !('Err' in value)) ||
      ('Err' in value && !('Ok' in value) && protocolError(value.Err)))
  );
}
function images(value: unknown): boolean {
  return (
    value === undefined ||
    (Array.isArray(value) &&
      value.every(
        (image) =>
          isRecord(image) &&
          ['image/png', 'image/jpeg', 'image/gif', 'image/webp'].includes(
            String(image.media_type),
          ) &&
          string(image.data) &&
          /^[A-Za-z0-9+/]*={0,2}$/.test(image.data),
      ))
  );
}
export function isAgentMessage(value: unknown): value is AgentMessage {
  if (!isRecord(value)) return false;
  switch (value.type) {
    case 'system':
      return string(value.content);
    case 'user':
      return string(value.content) && images(value.images);
    case 'assistant':
      return (
        nullableString(value.content) &&
        nullableString(value.reasoning) &&
        Array.isArray(value.tool_calls) &&
        value.tool_calls.every(
          (call) =>
            isRecord(call) && string(call.id) && string(call.tool_name) && 'arguments' in call,
        )
      );
    case 'tool_call':
      return (
        string(value.content) &&
        string(value.tool_call_id) &&
        string(value.tool_name) &&
        typeof value.failed === 'boolean' &&
        images(value.images)
      );
    case 'error':
      return string(value.message);
    case 'inject':
      return Array.isArray(value.messages) && value.messages.every(isAgentMessage);
    default:
      return false;
  }
}
export function parseServerMessage(raw: string): ServerMessage | null {
  try {
    const value: unknown = JSON.parse(raw);
    if (!isRecord(value) || !string(value.type)) return null;
    let valid = false;
    switch (value.type) {
      case 'connection.registered':
        valid =
          string(value.id) &&
          isRecord(value.client) &&
          string(value.client.resource_id) &&
          string(value.client.client_type) &&
          string(value.client.name);
        break;
      case 'state.sync':
        valid = backendState(value.state);
        break;
      case 'workspace.definitions':
        valid =
          string(value.id) &&
          Array.isArray(value.definitions) &&
          value.definitions.every(workspaceConfig);
        break;
      case 'workspace.started':
      case 'workspace.restarted':
        valid = string(value.id) && workspaceInfo(value.workspace);
        break;
      case 'workspace.stopped':
        valid = string(value.id) && workspaceReference(value.workspace);
        break;
      case 'connection.register_failed':
      case 'workspace.start_failed':
      case 'workspace.restart_failed':
      case 'workspace.stop_failed':
      case 'request.failed':
        valid = string(value.id) && protocolError(value.error);
        break;
      case 'agent.started':
      case 'agent.restarted':
        valid = string(value.id) && string(value.agent);
        break;
      case 'history.result':
        valid = string(value.id) && history(value.history);
        break;
      case 'mcl.command_result':
        valid =
          string(value.id) &&
          string(value.source) &&
          string(value.target) &&
          result(value.result) &&
          (value.history_error == null || protocolError(value.history_error));
        break;
      case 'agent.turn.abort_result':
        valid =
          string(value.id) &&
          workspaceReference(value.workspace) &&
          string(value.agent) &&
          string(value.turn_id) &&
          result(value.result) &&
          (!isRecord(value.result) || !('Ok' in value.result) || value.result.Ok === null);
        break;
      case 'agent.message.delta':
      case 'agent.message.reasoning_delta':
        valid =
          string(value.id) &&
          string(value.agent) &&
          string(value.agent_instance) &&
          integer(value.sequence) &&
          string(value.content);
        break;
      case 'agent.message.finished':
        valid =
          string(value.id) &&
          string(value.agent) &&
          string(value.agent_instance) &&
          (value.error === null || protocolError(value.error));
        break;
      case 'log':
        valid =
          isRecord(value.record) &&
          integer(value.record.timestamp_millis) &&
          string(value.record.level) &&
          string(value.record.target) &&
          string(value.record.message) &&
          Array.isArray(value.record.fields) &&
          value.record.fields.every(
            (field) => isRecord(field) && string(field.name) && string(field.value),
          );
        break;
    }
    return valid ? (value as unknown as ServerMessage) : null;
  } catch {
    return null;
  }
}
