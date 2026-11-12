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
  address: string;
  client_type: string;
}
export interface ProtocolError {
  code: string;
  message: string;
  details?: unknown;
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
export type AgentMessage = { sequence?: number } & (
  | { type: 'system'; content: string }
  | { type: 'user'; content: string; data?: UserImage[] }
  | {
      type: 'assistant';
      request_id?: string;
      content?: string | null;
      reasoning?: string | null;
      tool_calls?: ToolCall[];
      usage?: TokenUsage | null;
      stop_reason?: string | null;
    }
  | {
      type: 'tool_call';
      data?: UserImage[];
      tool_call_id: string;
      tool_name: string;
      content: string;
      failed: boolean;
    }
  | { type: 'error'; message: string; request_id?: string }
  | { type: 'inject'; messages: AgentMessage[] }
);
export interface AgentState {
  workspace: WorkspaceRef;
  agent: string;
  status: 'starting' | 'idle' | 'working' | 'restarting' | 'stopping';
  turn: string | null;
  sandbox: string | null;
  tools: string[];
  sandboxes: string[];
  queued_commands: number;
  pending_command: string | null;
  context_window_tokens?: number | null;
  error?: ProtocolError | null;
}
export interface HistoryMessage {
  sequence: number;
  turn: string | null;
  type?: string;
  source?: string;
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
  sequence: number;
  content: string;
}
export interface MclError {
  type: 'error';
  message: string;
  request_id: string;
}
export type MclValue =
  | 'Unit'
  | { Array: { Message: AgentMessage[] } }
  | { Config: Record<string, unknown> }
  | { Inference: { message: AgentMessage; stop_reason: string } }
  | { Import: { kind: 'Message' | 'Tool' | 'Sandbox'; alias: string } }
  | { Error: MclError };
export type MclCommandResult = { Ok: MclValue };
export type AbortResult = { Ok: null } | { Err: ProtocolError };
export interface WorkspaceSnapshot {
  path: string;
  definition: WorkspaceDefinition;
  status: 'inactive' | WorkspaceStatus | 'failed';
  address: string | null;
  error: ProtocolError | null;
}
export interface AgentTool {
  alias: string;
  path: string;
  description: string;
  input_schema: unknown;
  owner_task_id: string | null;
  binding: unknown;
}
export interface AgentSnapshot {
  address: string;
  workspace: string;
  status: 'starting' | 'ready' | 'working' | 'restarting' | 'stopping' | 'failed';
  tools: AgentTool[];
  error: ProtocolError | null;
}
export interface AgentTarget {
  address: string;
}
export interface TurnState extends AgentTarget {
  turn: string | null;
}
export interface TaskSnapshot {
  task_id: string;
  call_id: string;
  alias: string;
  returned: boolean;
  output: unknown[];
  error: ProtocolError | null;
}
export interface AgentLogRecord {
  sequence: number;
  created_at_ms: number;
  level: string;
  source: string;
  stage: string;
  message: string;
  details_json: unknown;
  context_json: unknown;
  origin: string | null;
  raw_error: string | null;
  traceback: string | null;
}
export interface AgentMclRecord {
  sequence: number;
  created_at_ms: number;
  source: string;
  turn: string | null;
  content: string;
  payload: unknown;
}
export interface RangeQuery {
  before_sequence?: number;
  after_sequence?: number;
  limit?: number;
}
export type MclData =
  | { kind: 'messages'; value: AgentMessage[] }
  | { kind: 'config'; value: Record<string, unknown> }
  | { kind: 'import'; value: { kind: string; alias: string } }
  | { kind: 'inference'; value: { message: AgentMessage; stop_reason: string } }
  | { kind: 'block_snapshots'; value: unknown };
export type ClientMessage =
  | { type: 'connection.register'; id: string; message: { client_type: string; name?: string } }
  | {
      type: 'workspace.req' | 'agent.req' | 'workspace.reload';
      id: string;
      message: Record<string, never>;
    }
  | {
      type: 'workspace.start' | 'workspace.restart' | 'workspace.stop';
      id: string;
      message: { path: string };
    }
  | { type: 'agent.start'; id: string; message: { workspace: string; name: string } }
  | {
      type: 'agent.stop' | 'agent.restart' | 'agent.turn.req' | 'agent.task.req';
      id: string;
      message: AgentTarget;
    }
  | {
      type: 'agent.history.req' | 'agent.log.req' | 'agent.mcl.req';
      id: string;
      message: AgentTarget & RangeQuery;
    }
  | {
      type: 'mcl.execute';
      id: string;
      message: AgentTarget & { command: string; arguments: unknown[] };
    }
  | { type: 'agent.turn.abort'; id: string; message: AgentTarget & { turn: string } };
type Failure = { success: false; error: ProtocolError };
type Receipt = { id: string } & ({ success: true } | Failure);
type Page<T> = { id: string } & AgentTarget &
  (
    | {
        success: true;
        records: T[];
        next_before_sequence: number | null;
        next_after_sequence: number | null;
      }
    | Failure
  );
export type ServerMessage =
  | ({ type: 'connection.register.result'; id: string } & (
      { success: true; client: { address: string; client_type: string } } | Failure
    ))
  | ({ type: 'workspace.res'; id: string } & (
      { success: true; workspaces: WorkspaceSnapshot[] } | Failure
    ))
  | { type: 'workspace.sync'; workspaces: WorkspaceSnapshot[] }
  | ({ type: 'agent.res'; id: string } & ({ success: true; agents: AgentSnapshot[] } | Failure))
  | { type: 'agent.sync'; agents: AgentSnapshot[] }
  | ({
      type:
        | 'workspace.start.result'
        | 'workspace.stop.result'
        | 'workspace.restart.result'
        | 'workspace.reload.result'
        | 'agent.start.result'
        | 'agent.stop.result'
        | 'agent.restart.result'
        | 'agent.turn.abort.result';
    } & Receipt)
  | ({ type: 'agent.turn.res'; id: string } & ({ success: true; state: TurnState } | Failure))
  | ({ type: 'agent.turn.sync' } & TurnState)
  | ({ type: 'agent.task.res'; id: string } & AgentTarget &
      ({ success: true; tasks: TaskSnapshot[] } | Failure))
  | ({ type: 'agent.task.sync'; tasks: TaskSnapshot[] } & AgentTarget)
  | ({ type: 'agent.history.res' } & Page<HistoryMessage>)
  | ({ type: 'agent.log.res' } & Page<AgentLogRecord>)
  | ({ type: 'agent.mcl.res' } & Page<AgentMclRecord>)
  | ({ type: 'agent.log.sync'; records: AgentLogRecord[] } & AgentTarget)
  | ({ type: 'agent.mcl.sync'; records: AgentMclRecord[] } & AgentTarget)
  | ({ type: 'mcl.execute.result'; id: string } & ({ success: true; data?: MclData } | Failure))
  | ({
      type: 'agent.message.chunk.sync';
      turn: string;
      generation: string;
      sequence: number;
      delta: { content?: string; reasoning?: string };
    } & AgentTarget)
  | ({
      type: 'agent.message.full.sync';
      turn: string | null;
      generation: string | null;
      history_sequence: number | null;
      message: AgentMessage;
      error: ProtocolError | null;
    } & AgentTarget);

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
const optionalNullableString = (value: unknown): boolean =>
  value === undefined || nullableString(value);
const integer = (value: unknown): boolean =>
  typeof value === 'number' && Number.isSafeInteger(value) && value >= 0;
const strings = (value: unknown): value is string[] => Array.isArray(value) && value.every(string);
const protocolError = (v: unknown): boolean => isRecord(v) && string(v.code) && string(v.message);
const target = (v: Record<string, unknown>): boolean => string(v.address);
const errorOrNull = (v: unknown): boolean => v === null || protocolError(v);
const definition = (v: unknown): boolean =>
  isRecord(v) &&
  string(v.name) &&
  string(v.project_root) &&
  Array.isArray(v.agents) &&
  v.agents.every((a) => isRecord(a) && string(a.name) && string(a.driver) && string(a.home_root));
const workspaceSnapshot = (v: unknown): boolean =>
  isRecord(v) &&
  string(v.path) &&
  definition(v.definition) &&
  ['inactive', 'starting', 'running', 'restarting', 'stopping', 'failed'].includes(
    String(v.status),
  ) &&
  nullableString(v.address) &&
  errorOrNull(v.error);
const agentSnapshot = (v: unknown): boolean =>
  isRecord(v) &&
  target(v) &&
  string(v.workspace) &&
  ['starting', 'ready', 'working', 'restarting', 'stopping', 'failed'].includes(String(v.status)) &&
  errorOrNull(v.error) &&
  Array.isArray(v.tools) &&
  v.tools.every(
    (t) =>
      isRecord(t) &&
      string(t.alias) &&
      string(t.path) &&
      string(t.description) &&
      isRecord(t.input_schema) &&
      nullableString(t.owner_task_id) &&
      'binding' in t,
  );
const tasks = (v: unknown): boolean =>
  Array.isArray(v) &&
  v.every(
    (t) =>
      isRecord(t) &&
      string(t.task_id) &&
      string(t.call_id) &&
      string(t.alias) &&
      typeof t.returned === 'boolean' &&
      Array.isArray(t.output) &&
      errorOrNull(t.error),
  );
const historyRecord = (v: unknown): boolean =>
  isRecord(v) &&
  integer(v.sequence) &&
  nullableString(v.turn) &&
  v.kind === 'message' &&
  string(v.content) &&
  integer(v.created_at_ms) &&
  isAgentMessage(v.payload) &&
  v.payload.sequence === v.sequence;
const logRecord = (v: unknown): boolean =>
  isRecord(v) &&
  integer(v.sequence) &&
  integer(v.created_at_ms) &&
  string(v.level) &&
  string(v.source) &&
  string(v.stage) &&
  string(v.message) &&
  'details_json' in v &&
  'context_json' in v &&
  nullableString(v.origin) &&
  nullableString(v.raw_error) &&
  nullableString(v.traceback);
const auditRecord = (v: unknown): boolean =>
  isRecord(v) &&
  integer(v.sequence) &&
  integer(v.created_at_ms) &&
  string(v.source) &&
  nullableString(v.turn) &&
  string(v.content) &&
  isRecord(v.payload);
const mclData = (v: unknown): boolean => {
  if (!isRecord(v)) return false;
  switch (v.kind) {
    case 'messages':
      return Array.isArray(v.value) && v.value.every(isAgentMessage);
    case 'config':
      return isRecord(v.value);
    case 'import':
      return isRecord(v.value) && string(v.value.kind) && string(v.value.alias);
    case 'inference':
      return isRecord(v.value) && isAgentMessage(v.value.message) && string(v.value.stop_reason);
    case 'block_snapshots':
      return isRecord(v.value);
    default:
      return false;
  }
};
export function isMclError(value: unknown): value is MclError {
  return (
    isRecord(value) && value.type === 'error' && string(value.message) && string(value.request_id)
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
  if (
    !isRecord(value) ||
    'replay' in value ||
    'images' in value ||
    (value.sequence !== undefined && !integer(value.sequence))
  )
    return false;
  switch (value.type) {
    case 'system':
      return string(value.content);
    case 'user':
      return string(value.content) && images(value.data);
    case 'assistant':
      return (
        optionalNullableString(value.content) &&
        optionalNullableString(value.reasoning) &&
        (value.tool_calls === undefined ||
          (Array.isArray(value.tool_calls) &&
            value.tool_calls.every(
              (call) =>
                isRecord(call) && string(call.id) && string(call.tool_name) && 'arguments' in call,
            )))
      );
    case 'tool_call':
      return (
        string(value.content) &&
        string(value.tool_call_id) &&
        string(value.tool_name) &&
        typeof value.failed === 'boolean' &&
        images(value.data)
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
    const v: unknown = JSON.parse(raw);
    if (!isRecord(v) || !string(v.type)) return null;
    const failure = v.success === false && protocolError(v.error);
    const reply = string(v.id) && (v.success === true ? v.error === undefined : failure);
    const page = (check: (item: unknown) => boolean) =>
      target(v) &&
      reply &&
      (v.success === false
        ? v.records === undefined &&
          v.next_before_sequence === undefined &&
          v.next_after_sequence === undefined
        : Array.isArray(v.records) &&
          v.records.every(check) &&
          (v.next_before_sequence === null || integer(v.next_before_sequence)) &&
          (v.next_after_sequence === null || integer(v.next_after_sequence)));
    let valid = false;
    switch (v.type) {
      case 'connection.register.result':
        valid =
          reply &&
          (v.success === false
            ? v.client === undefined
            : isRecord(v.client) && string(v.client.address) && string(v.client.client_type));
        break;
      case 'workspace.res':
        valid =
          reply &&
          (v.success === false
            ? v.workspaces === undefined
            : Array.isArray(v.workspaces) && v.workspaces.every(workspaceSnapshot));
        break;
      case 'workspace.sync':
        valid =
          v.id === undefined &&
          Array.isArray(v.workspaces) &&
          v.workspaces.every(workspaceSnapshot);
        break;
      case 'agent.res':
        valid =
          reply &&
          (v.success === false
            ? v.agents === undefined
            : Array.isArray(v.agents) && v.agents.every(agentSnapshot));
        break;
      case 'agent.sync':
        valid = v.id === undefined && Array.isArray(v.agents) && v.agents.every(agentSnapshot);
        break;
      case 'workspace.start.result':
      case 'workspace.stop.result':
      case 'workspace.restart.result':
      case 'workspace.reload.result':
      case 'agent.start.result':
      case 'agent.stop.result':
      case 'agent.restart.result':
      case 'agent.turn.abort.result':
        valid = reply;
        break;
      case 'agent.turn.res':
        valid =
          reply &&
          (v.success === false
            ? v.state === undefined
            : isRecord(v.state) && target(v.state) && nullableString(v.state.turn));
        break;
      case 'agent.turn.sync':
        valid = v.id === undefined && target(v) && nullableString(v.turn);
        break;
      case 'agent.task.res':
        valid =
          reply && target(v) && (v.success === false ? v.tasks === undefined : tasks(v.tasks));
        break;
      case 'agent.task.sync':
        valid = v.id === undefined && target(v) && tasks(v.tasks);
        break;
      case 'agent.history.res':
        valid = page(historyRecord);
        break;
      case 'agent.log.res':
        valid = page(logRecord);
        break;
      case 'agent.mcl.res':
        valid = page(auditRecord);
        break;
      case 'agent.log.sync':
        valid =
          v.id === undefined && target(v) && Array.isArray(v.records) && v.records.every(logRecord);
        break;
      case 'agent.mcl.sync':
        valid =
          v.id === undefined &&
          target(v) &&
          Array.isArray(v.records) &&
          v.records.every(auditRecord);
        break;
      case 'mcl.execute.result':
        valid =
          reply &&
          (v.success === false ? v.data === undefined : v.data === undefined || mclData(v.data));
        break;
      case 'agent.message.chunk.sync':
        valid =
          v.id === undefined &&
          target(v) &&
          string(v.turn) &&
          string(v.generation) &&
          integer(v.sequence) &&
          isRecord(v.delta) &&
          (string(v.delta.content) || string(v.delta.reasoning)) &&
          (v.delta.content === undefined || string(v.delta.content)) &&
          (v.delta.reasoning === undefined || string(v.delta.reasoning));
        break;
      case 'agent.message.full.sync':
        valid =
          v.id === undefined &&
          target(v) &&
          nullableString(v.turn) &&
          nullableString(v.generation) &&
          (v.history_sequence === null || integer(v.history_sequence)) &&
          isAgentMessage(v.message) &&
          (v.history_sequence === null || v.message.sequence === v.history_sequence) &&
          errorOrNull(v.error);
        break;
    }
    return valid ? (v as unknown as ServerMessage) : null;
  } catch {
    return null;
  }
}
