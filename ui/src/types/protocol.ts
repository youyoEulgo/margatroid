export interface WorkspaceRef {
  id: string;
  name: string;
  project_root: string;
}
export interface WorkspaceInfo extends WorkspaceRef {
  member_failures?: { agent: string; driver: string; error: string }[];
  agents: string[];
}
export interface AgentDefinition { id: string; name: string; driver: string; home_root?: string }
export interface WorkspaceDefinition extends WorkspaceRef { agents: AgentDefinition[] }
export interface CatalogEntry { id: string; name: string; path: string; revision: string; definition?: WorkspaceDefinition; error?: string; lifecycle: string }
export interface DriverCatalog { id: string; drivers: string[]; errors: { path: string; message: string }[] }
export interface WorkspacePaths { id: string; path: string; parent: string | null; directories: { name: string; path: string }[]; next_cursor?: string | null }
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
export interface UserImage { media_type: string; data: string }
export type AgentMessage =
  | { type: 'system'; content: string }
  | { type: 'user'; content: string; images?: UserImage[] }
  | {
      type: 'assistant';
      reasoning: string | null;
      content: string | null;
      tool_calls: ToolCall[];
      usage?: TokenUsage | null;
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
  status: 'creating' | 'idle' | 'working' | 'stopping';
  turn: string | null;
  sandbox: string | null;
  queued_commands: number;
  pending_command: string | null;
  resources?: string[];
  error?: ProtocolError | null;
  context_window_tokens?: number;
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
  catalog: CatalogEntry[];
  workspaces: WorkspaceInfo[];
  agents: AgentState[];
  histories: AgentHistory[];
  /**
   * Available agent driver resource IDs. Optional until the daemon ships the
   * driver listing; the UI falls back to the drivers already in the catalog.
   */
  drivers?: string[];
}

/**
 * Pending catalog wire. The request names are centralized here so the UI and
 * store share one definition; they are flipped on only after the daemon
 * confirms each request (see `SUPPORTED_CATALOG_REQUESTS`).
 */
export const CATALOG_REQUESTS = {
  createWorkspace: 'workspace.create',
  renameWorkspace: 'workspace.rename',
  deleteWorkspace: 'workspace.delete',
  renameMember: 'workspace.member.rename',
} as const;
export type CatalogRequestType = (typeof CATALOG_REQUESTS)[keyof typeof CATALOG_REQUESTS];
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
  | { type: 'driver.catalog.query' | 'driver.catalog.refresh'; id: string; message: Record<string, never> }
  | { type: 'workspace.paths.query'; id: string; message: { path?: string; cursor?: string } }
  | { type: 'workspace.catalog.query' | 'workspace.catalog.refresh'; id: string; message: Record<string, never> }
  | { type: 'workspace.start'; id: string; message: { workspace: string; expected_revision: string } }
  | { type: 'workspace.stop'; id: string; message: { workspace: WorkspaceRef } }
  | { type: 'workspace.member.add'; id: string; message: { workspace: string; expected_revision: string; agent: { name: string; driver: string; home_root?: string } } }
  | { type: 'workspace.member.remove'; id: string; message: { workspace: string; expected_revision: string; agent: string } }
  // Catalog mutations confirmed with the parent wire; every response reuses `workspace.member.result`.
  | { type: 'workspace.create'; id: string; message: { name: string; project_root: string } }
  | {
      type: 'workspace.rename';
      id: string;
      message: { workspace: string; expected_revision: string; new_name: string };
    }
  | { type: 'workspace.delete'; id: string; message: { workspace: string; expected_revision: string } }
  | {
      type: 'workspace.member.rename';
      id: string;
      message: { workspace: string; expected_revision: string; agent: string; new_name: string };
    }
  | {
      type: 'agent.turn.abort';
      id: string;
      message: { workspace: WorkspaceRef; agent: string; turn_id: string };
    }
  | { type: 'connection.register'; id: string; message: { client_type: string } }
  | { type: 'state.query'; id: string; message: Record<string, never> }
  | {
      type: 'history.query';
      id: string;
      message: { workspace: WorkspaceRef; agent: string };
    }
  | {
      type: 'mcl.command';
      id: string;
      message: {
        workspace: WorkspaceRef;
        agent: string;
        command: string;
        arguments: unknown[][];
      };
    };
export type ServerMessage =
  | ({ type: 'driver.catalog' } & DriverCatalog)
  | ({ type: 'workspace.paths' } & WorkspacePaths)
  | { type: 'workspace.catalog'; id: string; entries: CatalogEntry[] }
  | { type: 'workspace.member.result'; id: string; entry?: CatalogEntry | null; error?: ProtocolError | null }
  | {
      type: 'agent.turn.abort_result';
      id: string;
      workspace: WorkspaceRef;
      agent: string;
      turn_id: string;
      result: { Ok: null } | { Err: ProtocolError };
    }
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
function hasImageArray(value: unknown): boolean {
  return value === undefined || (Array.isArray(value) && value.every((image) => isRecord(image) &&
    ['image/png', 'image/jpeg', 'image/gif', 'image/webp'].includes(String(image.media_type)) &&
    typeof image.data === 'string' && /^[A-Za-z0-9+/]*={0,2}$/.test(image.data)));
}
export function isAgentMessage(value: unknown): value is AgentMessage {
  if (!isRecord(value)) return false;
  switch (value.type) {
    case 'system':
      return typeof value.content === 'string';
    case 'user':
      return typeof value.content === 'string' && hasImageArray(value.images);
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
        typeof value.failed === 'boolean' && hasImageArray(value.images)
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
      case 'driver.catalog':
        return typeof value.id === 'string' && Array.isArray(value.drivers) && value.drivers.every((driver) => typeof driver === 'string') &&
          Array.isArray(value.errors) && value.errors.every((error) => isRecord(error) && typeof error.path === 'string' && typeof error.message === 'string')
          ? value as unknown as ServerMessage : null;
      case 'workspace.paths':
        return typeof value.id === 'string' && typeof value.path === 'string' && (value.parent === null || typeof value.parent === 'string') &&
          Array.isArray(value.directories) && value.directories.every((directory) => isRecord(directory) && typeof directory.name === 'string' && typeof directory.path === 'string') &&
          (value.next_cursor == null || typeof value.next_cursor === 'string') ? value as unknown as ServerMessage : null;
      case 'workspace.catalog':
        return typeof value.id === 'string' && Array.isArray(value.entries) ? value as unknown as ServerMessage : null;
      case 'workspace.member.result':
        return typeof value.id === 'string' && (value.entry == null || isRecord(value.entry)) ? value as unknown as ServerMessage : null;
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
      case 'agent.turn.abort_result':
        return typeof value.id === 'string' &&
          isRecord(value.workspace) &&
          typeof value.agent === 'string' &&
          typeof value.turn_id === 'string' &&
          isRecord(value.result) &&
          (value.result.Ok === null || isRecord(value.result.Err))
          ? (value as unknown as ServerMessage)
          : null;
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
