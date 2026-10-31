/// <reference types="node" />
import { strict as assert } from 'node:assert';
import { test } from 'node:test';
import { createPinia, setActivePinia } from 'pinia';
import { createRenderer, h, markRaw, nextTick } from 'vue';
import SidebarTree from '../components/SidebarTree.vue';
import {
  buildSidebarModel,
  contextInjection,
  historyEntries,
  latestContextTokens,
  parseArguments,
  useWorkbenchStore,
} from './workbench';
import {
  isAgentMessage,
  parseServerMessage,
  type AgentHistory,
  type BackendState,
  type ServerMessage,
  type WorkspaceInfo,
} from '../types/protocol';
import { formatRecordTime } from '../lib/time';
import {
  imageMediaType,
  importImage,
  metadataText,
  validateImageBudget,
  wireImages,
} from '../lib/images';

const path = '/definitions/test/workspace.yaml';
const workspace: WorkspaceInfo = {
  id: 'workspace:local/runtime:latest',
  name: 'Runtime',
  project_root: '/test',
  definition_path: path,
  status: 'running',
  agents: ['agent:runtime/coder:latest'],
};
const state: BackendState = {
  workspace_definitions: [
    {
      path,
      cache: {
        name: 'Display test',
        project_root: '/test',
        agents: [{ name: 'coder', driver: 'driver:local/coder:latest', home_root: '/home/coder' }],
      },
    },
  ],
  workspaces: [workspace],
  agents: [
    {
      workspace,
      agent: workspace.agents[0]!,
      agent_instance: 'vm-current',
      status: 'idle',
      turn: null,
      sandbox: null,
      queued_commands: 0,
      pending_command: null,
      context_window_tokens: 8192,
    },
  ],
  histories: [],
};
function working(turn: string | null = 'turn-1'): BackendState {
  return { ...state, agents: [{ ...state.agents[0]!, status: 'working', turn }] };
}
interface Wire {
  type: string;
  id: string;
  message: Record<string, unknown>;
}
function setup(snapshot: BackendState = state) {
  setActivePinia(createPinia());
  const store = useWorkbenchStore();
  const original = globalThis.WebSocket;
  const sent: Wire[] = [];
  const sockets: Socket[] = [];
  class Socket {
    static OPEN = 1;
    readyState = 1;
    onopen: (() => void) | null = null;
    onclose: (() => void) | null = null;
    onmessage: ((event: { data: string }) => void) | null = null;
    onerror = null;
    constructor() {
      sockets.push(this);
    }
    close() {}
    send(raw: string) {
      sent.push(JSON.parse(raw) as Wire);
    }
  }
  globalThis.WebSocket = Socket as unknown as typeof WebSocket;
  store.handleServerMessage({ type: 'state.sync', state: snapshot });
  store.connect();
  sockets[0]!.onopen?.();
  store.handleServerMessage({
    type: 'connection.registered',
    id: sent[0]!.id,
    client: { resource_id: 'client:local/ui:latest', name: 'test', client_type: 'webui' },
  });
  const receive = (event: unknown) => sockets.at(-1)!.onmessage?.({ data: JSON.stringify(event) });
  const reply = (id: string, result: unknown = 'Unit', agent = workspace.agents[0]!) =>
    receive({
      type: 'mcl.command_result',
      id,
      source: 'client:local/ui:latest',
      target: agent,
      result: { Ok: result },
    });
  return {
    store,
    sent,
    receive,
    reply,
    cleanup() {
      store.disconnect();
      globalThis.WebSocket = original;
    },
  };
}
function delta(
  id = 'inference',
  sequence = 1,
  content = 'partial',
): Extract<ServerMessage, { type: 'agent.message.delta' | 'agent.message.reasoning_delta' }> {
  return {
    type: 'agent.message.delta',
    id,
    agent: workspace.agents[0]!,
    agent_instance: 'vm-current',
    sequence,
    content,
  };
}
function history(messages: AgentHistory['messages']): AgentHistory {
  return { workspace, agent: workspace.agents[0]!, messages };
}

test('wire accepts path/cache state and rejects removed catalog and malformed history', () => {
  assert.equal(
    parseServerMessage(JSON.stringify({ type: 'state.sync', state }))?.type,
    'state.sync',
  );
  for (const type of [
    'workspace.catalog',
    'workspace.member.result',
    'driver.catalog',
    'workspace.paths',
  ])
    assert.equal(parseServerMessage(JSON.stringify({ type, id: 'old', entries: [] })), null);
  assert.equal(
    parseServerMessage(
      JSON.stringify({
        type: 'state.sync',
        state: { catalog: [], workspaces: [], agents: [], histories: [] },
      }),
    ),
    null,
  );
  assert.equal(
    parseServerMessage(
      JSON.stringify({
        type: 'history.result',
        id: 'h',
        history: history([
          {
            key: 'x',
            sequence: -1,
            turn: null,
            kind: 'message',
            content: '',
            payload: {},
            created_at_ms: 0,
          },
        ]),
      }),
    ),
    null,
  );
  assert.equal(parseServerMessage(JSON.stringify({ ...delta(), sequence: 0.5 })), null);
  assert.equal(
    parseServerMessage(JSON.stringify({ type: 'workspace.restarted', id: 'r', workspace }))?.type,
    'workspace.restarted',
  );
});
test('placeholder arguments accept JSON values and message injection preserves images', () => {
  assert.deepEqual(parseArguments('["alias", {"key":1}, [2], null]'), [
    'alias',
    { key: 1 },
    [2],
    null,
  ]);
  assert.throws(() => parseArguments('{"not":"array"}'));
  const image = { media_type: 'image/png', data: 'iVBORw0KGgo=' };
  assert.deepEqual(contextInjection('', 'entry.pending[-1]', [image]), {
    command: 'INJECT ? TO entry.pending[-1]',
    arguments: [[{ type: 'user', content: '', images: [image] }]],
  });
  for (const destination of ['', 'x\nEFFECT wake', 'field', 'x.y;EFFECT wake'])
    assert.throws(() => contextInjection('text', destination));
});
test('definition paths remain selection keys and cached member names never create network addresses', async () => {
  const stopped: BackendState = { ...state, workspaces: [], agents: [] };
  const env = setup(stopped);
  try {
    assert.equal(env.store.selectedWorkspaceKey, path);
    assert.equal(env.store.selectedAgent, null);
    assert.equal(env.store.sidebarTree.workspaces[0]!.agents[0]!.id, null);
    await assert.rejects(
      env.store.executeMclCommand('GET entry.pending'),
      /running agent explicitly/,
    );
    assert.ok(!env.sent.some((request) => request.type === 'mcl.command'));
    assert.deepEqual(
      env.sent.map((request) => request.type),
      ['connection.register', 'state.query'],
    );
    for (const removed of [
      'catalog',
      'refreshCatalog',
      'createWorkspace',
      'renameWorkspace',
      'deleteWorkspace',
      'addMember',
      'removeMember',
      'queryDrivers',
      'queryPaths',
    ])
      assert.equal(removed in env.store, false);
  } finally {
    env.cleanup();
  }
});
test('start/restart use path and stop uses authoritative runtime address without optimistic removal', async () => {
  const env = setup({ ...state, workspaces: [], agents: [] });
  try {
    const starting = env.store.startWorkspace(path);
    const start = env.sent.at(-1)!;
    assert.equal(start.type, 'workspace.start');
    assert.deepEqual(start.message, { path });
    assert.equal(env.store.runtimeWorkspaces.length, 0);
    env.receive({ type: 'workspace.started', id: start.id, workspace });
    await starting;
    assert.equal(env.store.runtimeWorkspaces[0]!.id, workspace.id);
    const restarting = env.store.restartWorkspace(path);
    const restart = env.sent.at(-1)!;
    assert.deepEqual(restart.message, { path });
    env.receive({ type: 'workspace.restarted', id: restart.id, workspace });
    await restarting;
    const stopping = env.store.stopWorkspace(workspace.id);
    const stop = env.sent.at(-1)!;
    assert.deepEqual(stop.message, { workspace: workspace.id });
    assert.equal(env.store.runtimeWorkspaces.length, 1);
    env.receive({ type: 'workspace.stopped', id: stop.id, workspace });
    await stopping;
    assert.equal(env.store.runtimeWorkspaces.length, 1);
    env.receive({ type: 'state.sync', state: { ...state, workspaces: [], agents: [] } });
    assert.equal(env.store.runtimeWorkspaces.length, 0);
    assert.equal(env.store.selectedWorkspaceKey, path);
  } finally {
    env.cleanup();
  }
});
test('workspace failure waits for matching receipt and preserves authoritative runtime data', async () => {
  const env = setup();
  try {
    const stopping = env.store.stopWorkspace();
    const rejection = assert.rejects(stopping, /cleanup pending/);
    const request = env.sent.at(-1)!;
    env.receive({
      type: 'workspace.stop_failed',
      id: 'other',
      error: { kind: 'CleanupFailed', message: 'wrong' },
    });
    assert.equal(env.store.workspaceOperationPending, true);
    env.receive({
      type: 'workspace.stop_failed',
      id: request.id,
      error: { kind: 'CleanupFailed', message: 'cleanup pending' },
    });
    await rejection;
    assert.equal(env.store.runtimeWorkspaces[0]!.id, workspace.id);
  } finally {
    env.cleanup();
  }
});
test('multiple agents require an actual explicit target and late selection changes do not wake another agent', async () => {
  const reviewer = 'agent:runtime/reviewer:latest';
  const snapshot: BackendState = {
    ...state,
    workspaces: [{ ...workspace, agents: [...workspace.agents, reviewer] }],
    agents: [
      ...state.agents,
      { ...state.agents[0]!, agent: reviewer, agent_instance: 'reviewer-vm' },
    ],
  };
  const env = setup(snapshot);
  try {
    assert.equal(env.store.selectedAgent, null);
    await assert.rejects(env.store.executeMclCommand('GET entry.pending'), /explicitly/);
    env.store.selectAgent(workspace.agents[0]!);
    const sending = env.store.sendMessage('hello');
    const rejection = assert.rejects(sending, /changed/);
    const inject = env.sent.find((request) => request.type === 'mcl.command')!;
    assert.equal(inject.message.agent, workspace.agents[0]);
    assert.equal(inject.message.workspace, workspace.id);
    env.store.selectAgent(reviewer);
    env.store.selectAgent(workspace.agents[0]!);
    env.reply(inject.id);
    await rejection;
    assert.ok(!env.sent.some((request) => request.message.command === 'EFFECT wake'));
    env.store.selectAgent('agent:unadvertised/guessed:latest');
    assert.equal(env.store.selectedAgent, null);
  } finally {
    env.cleanup();
  }
});
test('composer waits for injection then sends new EFFECT wake syntax to the same explicit target', async () => {
  const env = setup();
  try {
    const stages: string[] = [];
    const sending = env.store.sendMessage(' hello ', [], (stage) => stages.push(stage));
    const inject = env.sent.find((request) => request.type === 'mcl.command')!;
    assert.deepEqual(inject.message, {
      workspace: workspace.id,
      agent: workspace.agents[0],
      command: 'INJECT ? TO entry.pending[-1]',
      arguments: [[{ type: 'user', content: 'hello' }]],
    });
    assert.deepEqual(stages, ['injecting']);
    env.reply(inject.id);
    await Promise.resolve();
    await Promise.resolve();
    const wake = env.sent.find((request) => request.message.command === 'EFFECT wake')!;
    assert.ok(wake);
    assert.deepEqual(stages, ['injecting', 'waking']);
    env.reply(wake.id);
    await sending;
    assert.ok(!env.sent.some((request) => String(request.message.command).startsWith('EMIT ')));
  } finally {
    env.cleanup();
  }
});
test('failed injection does not wake and retry keeps backend acknowledgement ordering', async () => {
  const env = setup();
  try {
    const sending = env.store.sendMessage('keep draft');
    const rejection = assert.rejects(sending, /not written/);
    const inject = env.sent.find((request) => request.type === 'mcl.command')!;
    env.receive({
      type: 'request.failed',
      id: inject.id,
      error: { kind: 'WriteFailed', message: 'not written' },
    });
    await rejection;
    assert.ok(!env.sent.some((request) => request.message.command === 'EFFECT wake'));
    assert.equal(env.store.visibleMessages.length, 0);
    const retry = env.store.sendMessage('retry');
    const latest = env.sent.filter((request) => request.type === 'mcl.command').at(-1)!;
    env.reply(latest.id);
    await Promise.resolve();
    await Promise.resolve();
    env.reply(env.sent.find((request) => request.message.command === 'EFFECT wake')!.id);
    await retry;
  } finally {
    env.cleanup();
  }
});
test('stream instance, request, sequence and terminal checks reject stale or duplicate deltas', () => {
  const env = setup();
  try {
    env.receive({ ...delta(), agent_instance: 'old-vm' });
    assert.equal(env.store.visibleMessages.length, 0);
    env.receive(delta());
    env.receive(delta());
    env.receive({ ...delta('inference', 2, 'thinking'), type: 'agent.message.reasoning_delta' });
    assert.equal(env.store.visibleMessages[0]!.content, 'partial');
    assert.equal(env.store.visibleMessages[0]!.thinking, 'thinking');
    env.receive({
      type: 'agent.message.finished',
      id: 'inference',
      agent: workspace.agents[0],
      agent_instance: 'vm-current',
      error: null,
    });
    env.receive(delta('inference', 3, 'late'));
    assert.equal(env.store.visibleMessages[0]!.content, 'partial');
    env.receive({
      type: 'state.sync',
      state: { ...state, agents: [{ ...state.agents[0]!, agent_instance: 'new-vm' }] },
    });
    assert.equal(env.store.visibleMessages.length, 0);
    env.receive(delta('unknown-old'));
    assert.equal(env.store.visibleMessages.length, 0);
  } finally {
    env.cleanup();
  }
});
test('abort freezes click-time known and unseen request streams until authoritative sync', async () => {
  const env = setup(working());
  try {
    env.receive(delta());
    env.receive({ ...delta('inference', 2, 'thought'), type: 'agent.message.reasoning_delta' });
    const promise = env.store.abortTurn();
    const request = env.sent.find((event) => event.type === 'agent.turn.abort')!;
    assert.deepEqual(request.message, {
      workspace: workspace.id,
      agent: workspace.agents[0],
      agent_instance: 'vm-current',
      turn_id: 'turn-1',
    });
    assert.equal(env.store.abortPending, true);
    env.receive(delta('inference', 3, 'late'));
    env.receive(delta('unseen'));
    assert.equal(env.store.visibleMessages.length, 1);
    assert.equal(env.store.visibleMessages[0]!.content, 'partial');
    env.receive({
      type: 'agent.turn.abort_result',
      id: request.id,
      workspace,
      agent: workspace.agents[0],
      turn_id: 'wrong',
      result: { Ok: null },
    });
    assert.equal(env.store.abortPending, true);
    env.receive({
      type: 'agent.turn.abort_result',
      id: request.id,
      workspace,
      agent: workspace.agents[0],
      turn_id: 'turn-1',
      result: { Ok: null },
    });
    await promise;
    assert.equal(env.store.selectedAgentIdle, false);
    env.receive({ type: 'state.sync', state: working(null) });
    assert.equal(env.store.abortPending, true);
    env.receive(delta('unseen', 2, 'late unseen'));
    assert.equal(env.store.visibleMessages.length, 1);
    env.receive({ type: 'state.sync', state });
    assert.equal(env.store.selectedAgentIdle, true);
    assert.equal(env.store.abortPending, false);
    env.receive({ type: 'state.sync', state: working('turn-2') });
    env.receive(delta('inference', 4, 'stale'));
    env.receive(delta('unseen', 3, 'stale'));
    env.receive(delta('fresh'));
    assert.equal(env.store.visibleMessages.length, 2);
    assert.equal(env.store.visibleMessages[0]!.content, 'partial');
  } finally {
    env.cleanup();
  }
});
test('abort rejection stays frozen through its receipt and releases only on matching state sync', async () => {
  for (const generic of [false, true]) {
    const env = setup(working());
    try {
      env.receive(delta());
      const promise = env.store.abortTurn();
      const rejection = assert.rejects(promise, /denied/);
      const request = env.sent.find((event) => event.type === 'agent.turn.abort')!;
      env.receive(
        generic
          ? {
              type: 'request.failed',
              id: request.id,
              error: { kind: 'StateRejected', message: 'denied' },
            }
          : {
              type: 'agent.turn.abort_result',
              id: request.id,
              workspace,
              agent: workspace.agents[0],
              turn_id: 'turn-1',
              result: { Err: { kind: 'StateRejected', message: 'denied' } },
            },
      );
      await rejection;
      env.receive(delta('inference', 2, 'blocked'));
      assert.equal(env.store.visibleMessages[0]!.content, 'partial');
      assert.equal(env.store.abortPending, true);
      env.receive({ type: 'state.sync', state: working() });
      assert.equal(env.store.abortPending, false);
      env.receive(delta('inference', 2, ' resumed'));
      assert.equal(env.store.visibleMessages[0]!.content, 'partial resumed');
      assert.equal(env.store.selectedAgentIdle, false);
    } finally {
      env.cleanup();
    }
  }
});
test('late abort receipt can finish original promise after new VM sync without changing new VM state', async () => {
  const env = setup(working());
  try {
    const promise = env.store.abortTurn();
    const request = env.sent.find((event) => event.type === 'agent.turn.abort')!;
    env.receive({
      type: 'state.sync',
      state: { ...state, agents: [{ ...state.agents[0]!, agent_instance: 'new-vm' }] },
    });
    env.receive({
      type: 'agent.turn.abort_result',
      id: request.id,
      workspace,
      agent: workspace.agents[0],
      turn_id: 'turn-1',
      result: { Ok: null },
    });
    await promise;
    assert.equal(env.store.selectedAgentState!.agent_instance, 'new-vm');
    assert.equal(env.store.selectedAgentIdle, true);
  } finally {
    env.cleanup();
  }
});
test('real history keys, sequence gaps and unknown timestamps survive nested message projection', () => {
  const payload = {
    type: 'inject',
    messages: [
      { type: 'system', content: 'system' },
      {
        type: 'tool_call',
        tool_call_id: 'call',
        tool_name: 'run',
        content: 'background task task-42',
        failed: false,
      },
    ],
  };
  const entries = historyEntries(
    history([
      {
        key: 'real-db-key',
        sequence: 41,
        turn: null,
        kind: 'message',
        content: '',
        payload,
        created_at_ms: 0,
      },
    ]),
  );
  assert.deepEqual(
    entries.map((entry) => entry.role),
    ['system', 'tool'],
  );
  for (const entry of entries) {
    assert.equal(entry.historyKey, 'real-db-key');
    assert.equal(entry.sequence, 41);
    assert.equal(entry.timestamp, 0);
    assert.deepEqual(entry.payload, payload);
  }
  assert.equal(formatRecordTime(entries[0]!.timestamp), 'Time unavailable');
});
test('history reconciles a stream by explicit request_id, never by matching text', () => {
  const env = setup();
  try {
    env.receive(delta('request-id'));
    const row = {
      key: 'db-key',
      sequence: 79,
      turn: 'turn',
      kind: 'message',
      content: 'partial',
      payload: {
        type: 'assistant',
        request_id: 'other-request',
        reasoning: null,
        content: 'partial',
        tool_calls: [],
      },
      created_at_ms: 123,
    };
    env.receive({ type: 'history.result', id: 'h', history: history([row]) });
    assert.equal(env.store.visibleMessages.length, 2);
    env.receive({
      type: 'history.result',
      id: 'h2',
      history: history([{ ...row, payload: { ...row.payload, request_id: 'request-id' } }]),
    });
    assert.equal(env.store.visibleMessages.length, 1);
    assert.equal(env.store.visibleMessages[0]!.sequence, 79);
    assert.equal(env.store.visibleMessages[0]!.historyKey, 'db-key');
  } finally {
    env.cleanup();
  }
});
test('history keeps unknown records and background task identifiers as plain text and metadata', () => {
  const entry = historyEntries(
    history([
      {
        key: 'task',
        sequence: 90,
        turn: null,
        kind: 'tool_task',
        content: 'Background task: task-42',
        payload: { task_id: 'task-42', completed: false },
        created_at_ms: 123,
      },
    ]),
  )[0]!;
  assert.equal(entry.role, 'record');
  assert.match(entry.content, /task-42/);
  assert.match(metadataText(entry.payload), /task-42/);
});
test('token totals and latest context usage come only from persisted assistant history', () => {
  const assistant = (key: string, sequence: number, input: number, output: number) => ({
    key,
    sequence,
    turn: null,
    kind: 'message',
    content: 'answer',
    created_at_ms: 123,
    payload: {
      type: 'assistant',
      reasoning: null,
      tool_calls: [],
      usage: { input_tokens: input, output_tokens: output, cache_hit_tokens: 3 },
    },
  });
  const saved = history([assistant('second', 42, 60, 4), assistant('first', 41, 10, 2)]);
  assert.equal(latestContextTokens(saved), 64);
  const env = setup({ ...state, histories: [saved] });
  try {
    assert.deepEqual(env.store.tokenUsage, { input: 70, output: 6, cached: 6 });
    assert.equal(env.store.contextWindowTokens, 8192);
    assert.equal(env.store.contextTokens, 64);
  } finally {
    env.cleanup();
  }
});
test('images keep wire-only metadata and reject unsupported formats or excessive budgets', async () => {
  const image = { media_type: 'image/png', data: 'iVBORw0KGgo=', name: 'private.png', size: 8 };
  assert.deepEqual(wireImages([image]), [{ media_type: image.media_type, data: image.data }]);
  assert.ok(!metadataText({ images: [image] }).includes(image.data));
  assert.equal(imageMediaType(new Uint8Array([137, 80, 78, 71, 13, 10, 26, 10])), 'image/png');
  assert.equal(imageMediaType(new TextEncoder().encode('<svg/>')), null);
  validateImageBudget(Array(8).fill(2 * 1024 * 1024));
  assert.throws(() => validateImageBudget(Array(9).fill(1)), /8 images/);
  assert.throws(() => validateImageBudget([5 * 1024 * 1024 + 1]), /5 MiB/);
  await assert.rejects(
    importImage(new File(['<svg/>'], 'fake.png', { type: 'image/png' })),
    /Invalid image/,
  );
  assert.equal(
    isAgentMessage({
      type: 'user',
      content: '',
      images: [{ media_type: 'image/svg+xml', data: 'abcd' }],
    }),
    false,
  );
});
test('multi-MiB image send waits for parsed receipts and diagnostics do not expose image data', async () => {
  const env = setup();
  try {
    const image = { media_type: 'image/png', data: 'A'.repeat(2 * 1024 * 1024) };
    const sending = env.store.sendMessage('', [image]);
    const inject = env.sent.find((request) => request.type === 'mcl.command')!;
    assert.ok(JSON.stringify(inject).length > 1024 * 1024);
    assert.deepEqual(inject.message.arguments, [[{ type: 'user', content: '', images: [image] }]]);
    env.reply(inject.id);
    await Promise.resolve();
    await Promise.resolve();
    const wake = env.sent.find((request) => request.message.command === 'EFFECT wake')!;
    env.reply(wake.id);
    await sending;
    assert.ok(
      !JSON.stringify(env.store.logs.filter((log) => log.target === 'ui.send')).includes(
        image.data,
      ),
    );
  } finally {
    env.cleanup();
  }
});
test('sidebar status uses runtime state and errors while preserving configured path identity', () => {
  const model = buildSidebarModel(
    state.workspace_definitions,
    state.workspaces,
    state.agents,
    new Map(),
    new Set([path]),
    path,
    workspace.agents[0]!,
  );
  assert.equal(model.workspaces[0]!.key, path);
  assert.equal(model.workspaces[0]!.address, workspace.id);
  assert.equal(model.workspaces[0]!.status, 'running');
  const failed = buildSidebarModel(
    state.workspace_definitions,
    state.workspaces,
    [{ ...state.agents[0]!, error: { kind: 'Failure', message: 'actual failure' } }],
    new Map(),
    new Set(),
    path,
    null,
  );
  assert.equal(failed.workspaces[0]!.status, 'error');
});
test('sidebar renders configured members without mutation menus and emits explicit runtime targets', async () => {
  class Host {
    parent: Host | null = null;
    children: Host[] = [];
    props: Record<string, unknown> = {};
    text = '';
    constructor(public type: string) {
      markRaw(this);
    }
  }
  const descendants = (node: Host): Host[] =>
    node.children.flatMap((child) => [child, ...descendants(child)]);
  const renderer = createRenderer<Host, Host>({
    createElement: (type) => new Host(type),
    createText: (text) => Object.assign(new Host('text'), { text }),
    createComment: (text) => Object.assign(new Host('comment'), { text }),
    setText: (node, text) => {
      node.text = text;
    },
    setElementText: (node, text) => {
      node.text = text;
      node.children = [];
    },
    parentNode: (node) => node.parent,
    nextSibling: (node) => node.parent?.children[node.parent.children.indexOf(node) + 1] ?? null,
    patchProp: (node, key, _old, value) => {
      node.props[key] = value;
    },
    insert: (node, parent, anchor) => {
      if (node.parent) node.parent.children.splice(node.parent.children.indexOf(node), 1);
      node.parent = parent;
      const index = anchor ? parent.children.indexOf(anchor) : -1;
      if (index < 0) parent.children.push(node);
      else parent.children.splice(index, 0, node);
    },
    remove: (node) => {
      if (node.parent) node.parent.children.splice(node.parent.children.indexOf(node), 1);
      node.parent = null;
    },
  });
  const model = buildSidebarModel(
    state.workspace_definitions,
    state.workspaces,
    state.agents,
    new Map(),
    new Set([path]),
    path,
    null,
  );
  const events: unknown[][] = [];
  const root = new Host('root');
  const app = renderer.createApp({
    render: () =>
      h(SidebarTree, {
        model,
        pending: false,
        offline: false,
        onSelectAgent: (...args: unknown[]) => events.push(args),
      }),
  });
  try {
    app.mount(root);
    await nextTick();
    const nodes = descendants(root);
    assert.ok(!nodes.some((node) => node.props.role === 'menuitem'));
    const button = nodes.find(
      (node) => node.type === 'button' && String(node.props.class).includes('tree-main--agent'),
    )!;
    (button.props.onClick as () => void)();
    assert.deepEqual(events, [[workspace.agents[0], path]]);
    const text = (node: Host): string => node.text + node.children.map(text).join('');
    assert.ok(!/New workspace|New session|Rename|Delete|Refresh/.test(text(root)));
  } finally {
    app.unmount();
  }
});
