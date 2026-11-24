export interface WorkspaceRef {
  id: string;
  name: string;
  project_root: string;
}
export type WorkspaceReference = WorkspaceRef | string;
export type WorkspaceStatus = 'running' | 'deleting' | 'error';
export interface WorkspaceInfo extends WorkspaceRef {
  definition_path: string;
  status: WorkspaceStatus;
  agents: string[];
  error?: ProtocolError | null;
  member_failures?: { agent: string; driver: string; error: string }[];
}
export interface AgentDefinition {
  name: string;
  driver: string;
  home_root: string;
}
export interface WorkspaceDefinition {
  /** Optional legacy display name; new definitions derive it from the address. */
  name?: string;
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
      type: 'assistant' | 'compact';
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
export interface AgentCreator {
  address: string;
  task_id: string;
}
export interface AgentState {
  workspace: WorkspaceRef;
  agent: string;
  creator?: AgentCreator | null;
  status: 'starting' | 'running' | 'closing';
  turn: string | null;
  sandbox: string | null;
  tools: string[];
  sandboxes: string[];
  model?: AgentModel | null;
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
  | { Turn: string }
  | { Error: MclError };
export type MclCommandResult = { Ok: MclValue };
export type AbortResult = { Ok: null } | { Err: ProtocolError };
export interface AgentRegistration {
  id: string;
  parent_node: string;
  node_name: string | null;
  alias: string | null;
  category: 'normal' | 'temporary' | 'archived';
  tool_path: string;
  tool_arguments: Record<string, unknown>;
  previous_node_name: string | null;
  previous_alias: string | null;
}
export interface WorkspaceSnapshot {
  address: string;
  alias: string;
  project_root: string;
  status: WorkspaceStatus;
  agents: AgentRegistration[];
}
export interface ResourceItem {
  address: string;
  type: 'driver' | 'tool' | 'prompt' | 'sandbox';
  scope: string;
  name: string;
  tag: string;
  path: string;
  tool_path: string | null;
}
export interface AgentTool {
  alias: string;
  path: string;
  description: string;
  input_schema: unknown;
  owner_task_id: string | null;
  binding: unknown;
}
export interface AgentSandbox {
  alias: string;
  path: string;
}
export interface AgentModel {
  id: string;
  model: string;
  provider: string;
  api_type: string;
  context_window_tokens: number;
  thinking: string | null;
  reasoning_effort: string | null;
}
export interface AgentInfo {
  address: string;
  workspace: string;
  creator: AgentCreator;
  status: 'starting' | 'running' | 'closing';
  tools: AgentTool[];
  sandboxes: AgentSandbox[];
  sandbox: string | null;
  model: AgentModel | null;
  error: ProtocolError | null;
}
export interface AgentTarget {
  address: string;
}
export interface AgentUiFile {
  path: string;
  media_type: string;
  data: string;
}
export interface AgentUiBundle {
  revision: string;
  entry: string;
  files: AgentUiFile[];
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
  | { kind: 'block_snapshots'; value: unknown }
  | { kind: 'turn'; value: string };
export type ClientMessage =
  | { type: 'connection.register'; id: string; message: { client_type: string; name?: string } }
  | {
      type: 'workspace.req' | 'agent.req' | 'resource.req';
      id: string;
      message:
        | { type?: 'driver' | 'tool' | 'prompt' | 'sandbox'; scope?: string; name?: string }
        | Record<string, never>;
    }
  | {
      type: 'workspace.create';
      id: string;
      message: { address: string; alias: string; project_root: string };
    }
  | { type: 'workspace.delete'; id: string; message: { address: string } }
  | { type: 'workspace.rename'; id: string; message: { address: string; alias: string } }
  | { type: 'agent.rename'; id: string; message: { address: string; alias: string } }
  | {
      type: 'agent.start';
      id: string;
      message:
        | { mode: 'existing'; workspace: string; node_name: string }
        | { mode: 'new'; workspace: string; tool_path: string; arguments: Record<string, unknown> };
    }
  | { type: 'agent.stop'; id: string; message: { address: string } }
  | { type: 'agent.archive'; id: string; message: { workspace: string; node_name: string } }
  | {
      type: 'agent.turn.req' | 'agent.task.req';
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
  | { type: 'agent.ui.req'; id: string; message: AgentTarget }
  | { type: 'agent.turn.abort'; id: string; message: AgentTarget & { turn: string } }
  | { type: 'model.config.req'; id: string; message: Record<string, never> }
  | { type: 'model.config.update'; id: string; message: { content: string } };
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
  | ({ type: 'resource.res'; id: string } & (
      { success: true; resources: ResourceItem[] } | Failure
    ))
  | ({ type: 'agent.res'; id: string } & ({ success: true; agents: AgentInfo[] } | Failure))
  | { type: 'agent.sync'; agents: AgentInfo[] }
  | ({
      type:
        | 'workspace.create.result'
        | 'workspace.delete.result'
        | 'workspace.rename.result'
        | 'agent.rename.result'
        | 'agent.start.result'
        | 'agent.stop.result'
        | 'agent.archive.result'
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
  | ({ type: 'agent.ui.res'; id: string } & AgentTarget &
      ({ success: true; ui: AgentUiBundle | null } | Failure))
  | {
      type: 'agent.ui.sync';
      address: string;
      ui: AgentUiBundle | null;
      error: ProtocolError | null;
    }
  | ({
      type: 'agent.message.chunk.sync';
      turn: string | null;
      generation: string;
      sequence: number;
      content: string;
      reasoning: string;
    } & AgentTarget)
  | ({
      type: 'agent.message.full.sync';
      turn: string | null;
      generation: string | null;
      history_sequence: number | null;
      message: AgentMessage;
      error: ProtocolError | null;
    } & AgentTarget)
  | ({ type: 'model.config.res'; id: string } & ({ success: true; content: string } | Failure))
  | { type: 'model.config.sync'; content: string | null; error: ProtocolError | null }
  | ({ type: 'model.config.update.result'; id: string } & ({ success: true } | Failure));

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
const protocolError = (v: unknown): boolean => isRecord(v) && string(v.code) && string(v.message);
const target = (v: Record<string, unknown>): boolean => string(v.address);
const errorOrNull = (v: unknown): boolean => v === null || protocolError(v);
const definition = (v: unknown): boolean =>
  isRecord(v) &&
  (v.name === undefined || string(v.name)) &&
  string(v.project_root) &&
  Array.isArray(v.agents) &&
  v.agents.every((a) => isRecord(a) && string(a.name) && string(a.driver) && string(a.home_root));
const agentRegistration = (v: unknown): boolean =>
  isRecord(v) &&
  string(v.id) &&
  string(v.parent_node) &&
  (v.node_name === null || string(v.node_name)) &&
  (v.alias === null || string(v.alias)) &&
  ['normal', 'temporary', 'archived'].includes(String(v.category)) &&
  string(v.tool_path) &&
  isRecord(v.tool_arguments) &&
  (v.previous_node_name === null || string(v.previous_node_name)) &&
  (v.previous_alias === null || string(v.previous_alias));
const workspaceSnapshot = (v: unknown): boolean =>
  isRecord(v) &&
  string(v.address) &&
  string(v.alias) &&
  string(v.project_root) &&
  ['running', 'deleting', 'error'].includes(String(v.status)) &&
  Array.isArray(v.agents) &&
  v.agents.every(agentRegistration);
const resourceItem = (v: unknown): boolean =>
  isRecord(v) &&
  string(v.address) &&
  ['driver', 'tool', 'prompt', 'sandbox'].includes(String(v.type)) &&
  string(v.scope) &&
  string(v.name) &&
  string(v.tag) &&
  string(v.path) &&
  (v.tool_path === null || string(v.tool_path));
const uiPath = (v: unknown): boolean =>
  string(v) &&
  v.length > 0 &&
  !v.includes('\\') &&
  !v.startsWith('/') &&
  v.split('/').every((part) => part !== '' && part !== '.' && part !== '..');
const agentUiBundle = (v: unknown): boolean =>
  isRecord(v) &&
  string(v.revision) &&
  /^sha256:[0-9a-f]{64}$/.test(v.revision) &&
  v.entry === 'index.html' &&
  Array.isArray(v.files) &&
  v.files.length > 0 &&
  v.files.every(
    (file) =>
      isRecord(file) &&
      uiPath(file.path) &&
      string(file.media_type) &&
      string(file.data) &&
      /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(file.data),
  ) &&
  v.files.some((file) => isRecord(file) && file.path === 'index.html');
const agentInfo = (v: unknown): boolean =>
  isRecord(v) &&
  target(v) &&
  string(v.workspace) &&
  isRecord(v.creator) &&
  target(v.creator) &&
  string(v.creator.task_id) &&
  ['starting', 'running', 'closing'].includes(String(v.status)) &&
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
  ) &&
  Array.isArray(v.sandboxes) &&
  v.sandboxes.every((s) => isRecord(s) && string(s.alias) && string(s.path)) &&
  nullableString(v.sandbox) &&
  (v.model === null ||
    (isRecord(v.model) &&
      string(v.model.id) &&
      string(v.model.model) &&
      string(v.model.provider) &&
      string(v.model.api_type) &&
      integer(v.model.context_window_tokens) &&
      nullableString(v.model.thinking) &&
      nullableString(v.model.reasoning_effort)));
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
    case 'turn':
      return string(v.value);
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
    case 'compact':
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
      case 'resource.res':
        valid =
          reply &&
          (v.success === false
            ? v.resources === undefined
            : Array.isArray(v.resources) && v.resources.every(resourceItem));
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
            : Array.isArray(v.agents) && v.agents.every(agentInfo));
        break;
      case 'agent.sync':
        valid = v.id === undefined && Array.isArray(v.agents) && v.agents.every(agentInfo);
        break;
      case 'workspace.create.result':
      case 'workspace.delete.result':
      case 'workspace.rename.result':
      case 'agent.rename.result':
      case 'agent.start.result':
      case 'agent.stop.result':
      case 'agent.archive.result':
      case 'agent.turn.abort.result':
        valid = reply;
        break;
      case 'agent.ui.res':
        valid =
          reply &&
          target(v) &&
          (v.success === false ? v.ui === undefined : v.ui === null || agentUiBundle(v.ui));
        break;
      case 'agent.ui.sync':
        valid =
          v.id === undefined &&
          target(v) &&
          (v.ui === null || agentUiBundle(v.ui)) &&
          errorOrNull(v.error);
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
          nullableString(v.turn) &&
          string(v.generation) &&
          integer(v.sequence) &&
          string(v.content) &&
          string(v.reasoning);
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
      case 'model.config.res':
        valid = reply && (v.success === false ? v.content === undefined : string(v.content));
        break;
      case 'model.config.sync':
        valid = v.id === undefined && nullableString(v.content) && errorOrNull(v.error);
        break;
      case 'model.config.update.result':
        valid = reply;
        break;
    }
    return valid ? (v as unknown as ServerMessage) : null;
  } catch {
    return null;
  }
}
