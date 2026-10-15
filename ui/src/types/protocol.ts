export interface WorkspaceRef {
  id: string;
  name: string;
  project_root: string;
}
export interface WorkspaceInfo extends WorkspaceRef {
  manager: string;
  agents: string[];
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
  cache_hit_tokens?: number;
  cache_miss_tokens?: number;
}
export interface ToolCall {
  id: string;
  tool_name: string;
  arguments: string;
}
export type AgentMessage =
  | { type: 'system'; content: string }
  | { type: 'user'; content: string }
  | {
      type: 'assistant';
      reasoning: string | null;
      content: string | null;
      tool_calls: ToolCall[];
      usage?: TokenUsage | null;
    }
  | {
      type: 'tool_call';
      resource_id: string | null;
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
  status: 'creating' | 'running' | 'stopping';
  turn: string | null;
  sandbox: string | null;
  queued_commands: number;
  pending_command: string | null;
  resources?: string[];
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
  workspaces: WorkspaceInfo[];
  agents: AgentState[];
  histories: AgentHistory[];
}
export interface LogRecord {
  timestamp_millis: number;
  level: string;
  target: string;
  message: string;
  fields: Array<{ name: string; value: string }>;
  spans: string[];
}
export interface StreamDelta {
  id: string;
  agent: string;
  agent_instance: string;
  sequence: number;
  content: string;
}
export interface CallableResourceProjection {
  resource_id: string;
  alias: string;
  description: string;
  input_schema: unknown;
}
export interface SandboxProjection {
  resource_id: string;
}
export type BlockInnerProjection =
  | { Message: AgentMessage[] }
  | { Tool: CallableResourceProjection[] }
  | { Sandbox: SandboxProjection[] };
export type MclValueProjection =
  | 'Unit'
  | { Array: BlockInnerProjection }
  | { BlockOpened: { block_id: string; created: boolean; temporary: boolean } }
  | { ResourceImport: { kind: 'Message' | 'Tool' | 'Sandbox'; resource_id: string; alias: string } }
  | { Inference: { message: AgentMessage; stop_reason: unknown } }
  | { Text: string };
export type MclCommandResult = { Ok: MclValueProjection } | { Err: ProtocolError };
export type ClientMessage =
  | { type: 'connection.register'; id: string; message: { client_type: string } }
  | { type: 'state.query'; id: string; message: Record<string, never> }
  | {
      type: 'history.query';
      id: string;
      message: { workspace: WorkspaceRef; agent: string | null };
    }
  | {
      type: 'mcl.command';
      id: string;
      message: {
        workspace: WorkspaceRef;
        agent: string | null;
        command: string;
        arguments: unknown[][];
      };
    };
export type ServerMessage =
  | { type: 'connection.registered'; id: string; client: ClientInfo }
  | { type: 'connection.register_failed'; id: string; error: ProtocolError }
  | { type: 'state.sync'; state: BackendState }
  | { type: 'workspace.started'; id: string; workspace: WorkspaceInfo }
  | { type: 'workspace.stopped'; id: string; workspace: WorkspaceRef }
  | { type: 'workspace.start_failed' | 'workspace.stop_failed'; id: string; error: ProtocolError }
  | {
      type: 'mcl.command_result';
      id: string;
      source: string;
      target: string;
      result: MclCommandResult;
      history_error: ProtocolError | null;
    }
  | ({ type: 'agent.message.delta' | 'agent.message.reasoning_delta' } & StreamDelta)
  | { type: 'history.result'; id: string; history: AgentHistory }
  | { type: 'request.failed'; id: string; error: ProtocolError }
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
export function isAgentMessage(value: unknown): value is AgentMessage {
  if (!isRecord(value)) return false;
  switch (value.type) {
    case 'system':
    case 'user':
      return typeof value.content === 'string';
    case 'assistant':
      return (
        (value.content === null || typeof value.content === 'string') &&
        (value.reasoning === null || typeof value.reasoning === 'string') &&
        Array.isArray(value.tool_calls)
      );
    case 'tool_call':
      return (
        typeof value.content === 'string' &&
        typeof value.tool_call_id === 'string' &&
        typeof value.failed === 'boolean'
      );
    case 'error':
      return typeof value.message === 'string';
    case 'inject':
      return Array.isArray(value.messages) && value.messages.every(isAgentMessage);
    default:
      return false;
  }
}
export function parseServerMessage(raw: string): ServerMessage | null {
  try {
    const value: unknown = JSON.parse(raw);
    if (!isRecord(value) || typeof value.type !== 'string') return null;
    switch (value.type) {
      case 'state.sync':
        return isRecord(value.state) &&
          Array.isArray(value.state.workspaces) &&
          Array.isArray(value.state.agents) &&
          Array.isArray(value.state.histories)
          ? (value as unknown as ServerMessage)
          : null;
      case 'connection.registered':
        return typeof value.id === 'string' &&
          isRecord(value.client) &&
          typeof value.client.resource_id === 'string'
          ? (value as unknown as ServerMessage)
          : null;
      case 'connection.register_failed':
      case 'workspace.start_failed':
      case 'workspace.stop_failed':
      case 'request.failed':
        return typeof value.id === 'string' && isRecord(value.error)
          ? (value as unknown as ServerMessage)
          : null;
      case 'workspace.started':
      case 'workspace.stopped':
        return isRecord(value.workspace) ? (value as unknown as ServerMessage) : null;
      case 'mcl.command_result':
        return typeof value.id === 'string' &&
          isRecord(value.result) &&
          ('Ok' in value.result || 'Err' in value.result)
          ? (value as unknown as ServerMessage)
          : null;
      case 'agent.message.delta':
      case 'agent.message.reasoning_delta':
        return typeof value.id === 'string' &&
          typeof value.agent === 'string' &&
          typeof value.agent_instance === 'string' &&
          Number.isSafeInteger(value.sequence) &&
          typeof value.content === 'string'
          ? (value as unknown as ServerMessage)
          : null;
      case 'history.result':
        return typeof value.id === 'string' &&
          isRecord(value.history) &&
          Array.isArray(value.history.messages)
          ? (value as unknown as ServerMessage)
          : null;
      case 'agent.message.finished':
        return typeof value.id === 'string' &&
          typeof value.agent_instance === 'string' &&
          (value.error === null || isRecord(value.error))
          ? (value as unknown as ServerMessage)
          : null;
      case 'log':
        return isRecord(value.record) ? (value as unknown as ServerMessage) : null;
      default:
        return null;
    }
  } catch {
    return null;
  }
}
