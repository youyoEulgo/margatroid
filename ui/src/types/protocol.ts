export interface WorkspaceRef {
  id: string;
  name: string;
  project_root: string;
}

export interface ClientInfo {
  resource_id: string;
  client_type: string;
  name: string;
}

export interface WorkspaceInfo extends WorkspaceRef {
  manager: string;
  agents: string[];
}

export type ResourceRef = string;
export type AgentStatus = 'creating' | 'ready' | 'failed';

export interface BlockPath {
  block_id: string;
  inner_id: string;
}

export interface AgentState {
  workspace: WorkspaceRef;
  agent: string;
  status: AgentStatus;
  working: boolean;
  error: string | null;
  default_resources: ResourceRef[];
  visible_resources: ResourceRef[];
  default_visibility_source: BlockPath | null;
  visibility_source: BlockPath | null;
  resources: AgentResource[];
  exposed: Record<string, Record<string, unknown>>;
  mcl: AgentMclState | null;
  total_input_tokens: number;
  total_output_tokens: number;
  total_cache_hit_tokens: number;
  cache_hit_rate: number;
  last_input_tokens: number;
  context_window_tokens: number;
}

export interface AgentResource {
  resource_id: ResourceRef;
  resource_name: string;
}

export interface AgentMclState {
  base: ResourceRef;
  base_program_hash: string;
  plan_hash: string;
  plan_generation: number;
}

export interface HistoryMessage {
  sequence: number;
  turn_id: string;
  message: AgentMessage;
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

export interface ToolCall {
  id: string;
  tool_name: string;
  arguments: string;
}

export type AgentMessage =
  | { User: { content: string } }
  | { Assistant: { reasoning: string | null; content: string | null; tool_calls: ToolCall[] } }
  | {
      Tool: {
        resource_id: ResourceRef;
        tool_call_id: string;
        content: string;
        failed: boolean;
      };
    }
  | { Error: { message: string } }
  | { Inject: { messages: AgentMessage[] } };

export interface LogField {
  name: string;
  value: string;
}

export interface LogRecord {
  timestamp_millis: number;
  level: string;
  target: string;
  message: string;
  fields: LogField[];
  spans: string[];
}

export type MclCommandResult = { Ok: unknown } | { Err: string };

export type ClientMessage =
  | {
      type: 'connection.register';
      id: string;
      message: {
        client_type: string;
      };
    }
  | {
      type: 'agent.message';
      id: string;
      message: {
        workspace: WorkspaceRef;
        agent: string | null;
        message: AgentMessage;
      };
    }
  | {
      type: 'mcl.command';
      id: string;
      message: {
        workspace: WorkspaceRef;
        agent: string | null;
        command: string;
        binding: unknown;
      };
    }
  | {
      type: 'agent.turn.abort';
      id: string;
      message: {
        workspace: WorkspaceRef;
        agent: string | null;
      };
    };

export type ServerMessage =
  | { type: 'log'; record: LogRecord }
  | { type: 'state.sync'; state: BackendState }
  | { type: 'connection.registered'; id: string; client: ClientInfo }
  | { type: 'connection.register_failed'; id: string; error: string }
  | { type: 'workspace.started'; id: string; workspace: WorkspaceInfo }
  | { type: 'workspace.start_failed'; id: string; error: string }
  | {
      type: 'agent.message';
      message: {
        id: string;
        workspace: WorkspaceRef;
        agent: string;
        message: AgentMessage;
      };
    }
  | {
      type: 'mcl.command_result';
      id: string;
      result: MclCommandResult;
    }
  | {
      type: 'agent.message.delta';
      id: string;
      agent: string;
      content: string;
    }
  | {
      type: 'agent.message.reasoning_delta';
      id: string;
      agent: string;
      content: string;
    }
  | {
      type: 'agent.failure';
      failure: {
        id: string;
        workspace: WorkspaceRef;
        agent: string;
        kind: string;
        message: string;
      };
    };

export function parseServerMessage(raw: string): ServerMessage | null {
  try {
    const value: unknown = JSON.parse(raw);
    if (!isRecord(value) || typeof value.type !== 'string') return null;

    switch (value.type) {
      case 'log':
        return isRecord(value.record) ? (value as ServerMessage) : null;
      case 'state.sync':
        if (!isRecord(value.state) || !Array.isArray(value.state.workspaces)) return null;
        return {
          type: 'state.sync',
          state: {
            workspaces: value.state.workspaces as WorkspaceInfo[],
            agents: Array.isArray(value.state.agents) ? (value.state.agents as AgentState[]) : [],
            histories: Array.isArray(value.state.histories)
              ? (value.state.histories as AgentHistory[])
              : [],
          },
        };
      case 'connection.registered':
        return typeof value.id === 'string' &&
          isRecord(value.client) &&
          typeof value.client.resource_id === 'string' &&
          typeof value.client.client_type === 'string' &&
          typeof value.client.name === 'string'
          ? (value as ServerMessage)
          : null;
      case 'connection.register_failed':
        return typeof value.id === 'string' && typeof value.error === 'string'
          ? (value as ServerMessage)
          : null;
      case 'workspace.started':
        return isRecord(value.workspace) ? (value as ServerMessage) : null;
      case 'workspace.start_failed':
        return typeof value.id === 'string' && typeof value.error === 'string'
          ? (value as ServerMessage)
          : null;
      case 'agent.message':
        return isRecord(value.message) ? (value as ServerMessage) : null;
      case 'mcl.command_result':
        return typeof value.id === 'string' && isMclCommandResult(value.result)
          ? (value as ServerMessage)
          : null;
      case 'agent.message.delta':
      case 'agent.message.reasoning_delta':
        return typeof value.id === 'string' &&
          typeof value.agent === 'string' &&
          typeof value.content === 'string'
          ? (value as ServerMessage)
          : null;
      case 'agent.failure':
        return isRecord(value.failure) ? (value as ServerMessage) : null;
      default:
        return null;
    }
  } catch {
    return null;
  }
}

function isMclCommandResult(value: unknown): value is MclCommandResult {
  if (!isRecord(value)) return false;
  return Object.prototype.hasOwnProperty.call(value, 'Ok') || typeof value.Err === 'string';
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}
