/// <reference types="node" />
import { strict as assert } from 'node:assert';
import { test } from 'node:test';
import { createPinia, setActivePinia } from 'pinia';
import { createRenderer, h, markRaw, nextTick } from 'vue';
import SidebarTree from '../components/SidebarTree.vue';
import {
  buildSidebarModel,
  contextInjection,
  parseArguments,
  historyEntries,
  latestContextTokens,
  useWorkbenchStore,
} from './workbench';
import {
  isAgentMessage,
  parseServerMessage,
  type AgentHistory,
  type WorkspaceSnapshot,
  type AgentInfo,
  type TaskSnapshot,
} from '../types/protocol';
import { formatToolPayload, inlineToolPayload } from '../lib/format';
import {
  validateImageBudget,
  wireImages,
  metadataText,
  imageMediaType,
  importImage,
} from '../lib/images';

const path = '/definitions/test/workspace.yaml';
const address = 'agent:demo/coder:latest';
const workspace = 'workspace:local/demo:latest';
const ws: WorkspaceSnapshot = {
  path,
  definition: {
    name: 'Demo',
    project_root: '/project',
    agents: [{ name: 'coder', driver: 'driver:local/coder:latest', home_root: '/home/coder' }],
  },
  status: 'running',
  address: workspace,
  error: null,
};
const agent: AgentInfo = {
  address,
  workspace,

  status: 'ready',
  tools: [
    {
      alias: 'read_file',
      path: '/tools/read_file/tool.toml',
      description: 'Read a file',
      input_schema: { type: 'object' },
      owner_task_id: null,
      binding: null,
    },
  ],
  sandboxes: [
    { alias: 'workspace_write', path: '/sandboxes/workspace-write/sandbox.toml' },
    { alias: 'read_only', path: '/sandboxes/read-only/sandbox.toml' },
  ],
  sandbox: 'workspace_write',
  model: {
    id: 'default/model',
    model: 'model-name',
    provider: 'openai',
    api_type: 'chat_completions',
    context_window_tokens: 131072,
    thinking: null,
    reasoning_effort: null,
  },
  error: null,
};
interface Wire {
  type: string;
  id: string;
  message: Record<string, unknown>;
}
function setup(workspaces: WorkspaceSnapshot[] = [ws], agents: AgentInfo[] = [agent]) {
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
    onerror: (() => void) | null = null;
    constructor() {
      sockets.push(this);
    }
    close() {}
    send(raw: string) {
      sent.push(JSON.parse(raw));
    }
  }
  globalThis.WebSocket = Socket as unknown as typeof WebSocket;
  const receive = (event: unknown) => sockets.at(-1)!.onmessage?.({ data: JSON.stringify(event) });
  store.connect();
  sockets[0]!.onopen?.();
  receive({
    type: 'connection.register.result',
    id: sent[0]!.id,
    success: true,
    client: { address: 'client:webui/test:1', client_type: 'webui' },
  });
  receive({
    type: 'workspace.res',
    id: sent.find((x) => x.type === 'workspace.req')!.id,
    success: true,
    workspaces,
  });
  receive({
    type: 'agent.res',
    id: sent.find((x) => x.type === 'agent.req')!.id,
    success: true,
    agents,
  });
  const reply = (id: string, value?: unknown) =>
    receive({
      type: 'mcl.execute.result',
      id,
      success: true,
      ...(value === undefined ? {} : { data: value }),
    });
  return {
    store,
    sent,
    sockets,
    receive,
    reply,
    cleanup() {
      store.disconnect();
      globalThis.WebSocket = original;
    },
  };
}
function full(
  sequence: number | null = 42,
  generation: string | null = 'generation-1',
  content = 'complete',
) {
  return {
    type: 'agent.message.full.sync',
    address,

    turn: 'turn-1',
    generation,
    history_sequence: sequence,
    message: {
      type: 'assistant',
      ...(sequence === null ? {} : { sequence }),
      request_id: generation,
      content,
      reasoning: 'thought',
      tool_calls: [],
    },
    error: null,
  };
}
function chunk(sequence = 1, delta: Record<string, string> = { content: 'part' }) {
  return {
    type: 'agent.message.chunk.sync',
    address,

    turn: 'turn-1',
    generation: 'generation-1',
    sequence,
    delta,
  };
}
function historyRecord(sequence: number, content: string) {
  return {
    sequence,
    turn: null,
    kind: 'message',
    content,
    payload: { type: 'user', sequence, content },
    created_at_ms: 1000 + sequence,
  };
}
function page(id: string, records: unknown[]) {
  return {
    type: 'agent.history.res',
    id,
    success: true,
    address,

    records,
    next_before_sequence: records.length ? 10 : null,
    next_after_sequence: null,
  };
}

test('registration independently queries Workspace and active Agents without global state', () => {
  const e = setup();
  try {
    assert.deepEqual(
      e.sent.slice(0, 3).map((x) => x.type),
      ['connection.register', 'workspace.req', 'agent.req'],
    );
    assert.equal(e.store.registeredClient?.address, 'client:webui/test:1');
    const registration = e.sent[0]!;
    assert.equal(registration.type, 'connection.register');
    assert.match(
      String((registration.message as { name?: string }).name),
      /^webui-[0-9a-z-]+$/,
    );
    assert.equal(e.store.selectedAgent, address);
    assert.deepEqual(e.store.toolOptions, ['read_file']);
    assert.equal(
      e.store.toolDefinitions.get(`${address}`)?.[0]?.description,
      'Read a file',
    );
    assert.ok(!e.sent.some((x) => x.type.includes('state')));
  } finally {
    e.cleanup();
  }
});
test('a command only settles on its own result type', async () => {
  const e = setup();
  try {
    const pending = e.store.executeMclCommand('CONFIG GET');
    const command = e.sent.at(-1)!;
    assert.equal(command.type, 'mcl.execute');
    e.receive({ type: 'workspace.start.result', id: command.id, success: true });
    await assert.rejects(
      pending,
      /Received workspace.start.result for a waiting mcl.execute.result/,
    );
  } finally {
    e.cleanup();
  }
});
test('a workspace failure is carried into the sidebar row', () => {
  const e = setup();
  try {
    e.receive({
      type: 'workspace.sync',
      workspaces: [
        {
          ...ws,
          status: 'failed',
          error: { code: 'StartFailed', message: 'workspace failed to start' },
        },
      ],
    });
    const row = e.store.sidebarTree.workspaces[0]!;
    assert.equal(row.status, 'error');
    assert.equal(row.reason, 'workspace failed to start');
  } finally {
    e.cleanup();
  }
});
test('removing an agent releases its task snapshot', () => {
  const e = setup();
  try {
    e.receive({
      type: 'agent.task.sync',
      address,
      tasks: [
        {
          task_id: 'task-1',
          call_id: 'call-1',
          alias: 'bash',
          returned: true,
          output: ['ready'],
          error: null,
        },
      ],
    });
    assert.equal(e.store.taskStates.get(address)?.length, 1);
    e.receive({ type: 'agent.sync', agents: [] });
    assert.equal(e.store.taskStates.size, 0);
  } finally {
    e.cleanup();
  }
});
test('wire rejects removed aliases and legacy result envelopes', () => {
  for (const type of [
    'state.sync',
    'state.query',
    'workspace.definitions',
    'workspace.started',
    'connection.registered',
    'agent.message.delta',
    'agent.message.finished',
    'mcl.command_result',
    'history.result',
    'log',
  ])
    assert.equal(
      parseServerMessage(JSON.stringify({ type, id: 'old', state: {}, result: { Ok: 'Unit' } })),
      null,
    );
  assert.equal(
    parseServerMessage(
      JSON.stringify({ type: 'mcl.execute.result', id: 'x', result: { Ok: 'Unit' } }),
    ),
    null,
  );
});
test('Workspace sync includes inactive definitions and own status only', () => {
  const e = setup();
  try {
    e.receive({
      type: 'workspace.sync',
      workspaces: [{ ...ws, status: 'inactive', address: null }],
    });
    assert.equal(e.store.definitions.length, 1);
    assert.equal(e.store.runtimeWorkspaces.length, 0);
    assert.equal(e.store.sidebarTree.workspaces[0]?.name, 'Demo');
    assert.equal(e.store.selectedAgent, null);
  } finally {
    e.cleanup();
  }
});
test('Agent sync replaces full active collection and keeps full tool properties', () => {
  const e = setup();
  try {
    const dynamic = {
      ...agent.tools[0]!,
      alias: 'dynamic',
      owner_task_id: 'owner',
      binding: { server: 'test' },
    };
    e.receive({ type: 'agent.sync', agents: [{ ...agent, tools: [dynamic] }] });
    assert.deepEqual(e.store.toolOptions, ['dynamic']);
    assert.deepEqual(e.store.toolDefinitions.get(`${address}`)?.[0]?.binding, {
      server: 'test',
    });
    e.receive({ type: 'agent.sync', agents: [] });
    assert.equal(e.store.selectedAgent, null);
  } finally {
    e.cleanup();
  }
});
test('turn state is independent of Agent state and uses Agent Address', () => {
  const e = setup();
  try {
    e.receive({ type: 'agent.sync', agents: [{ ...agent, status: 'working' }] });
    assert.equal(e.store.canAbortTurn, false);
    e.receive({ type: 'agent.turn.sync', address, turn: 'turn-1' });
    assert.equal(e.store.canAbortTurn, true);
    e.receive({ type: 'agent.turn.sync', address: 'agent:demo/other:latest', turn: null });
    assert.equal(e.store.canAbortTurn, true);
  } finally {
    e.cleanup();
  }
});
test('MCL executes against explicit address and unwraps business data', async () => {
  const e = setup();
  try {
    const value = e.store.executeMclCommand('GET session.conversation');
    const wire = e.sent.at(-1)!;
    assert.equal(wire.type, 'mcl.execute');
    assert.deepEqual(wire.message, {
      address,

      command: 'GET session.conversation',
      arguments: [],
    });
    e.reply(wire.id, {
      kind: 'messages',
      value: [{ type: 'user', content: 'hello', sequence: 2 }],
    });
    assert.deepEqual(await value, {
      Array: { Message: [{ type: 'user', content: 'hello', sequence: 2 }] },
    });
  } finally {
    e.cleanup();
  }
});
test('MCL Unit success and failure resolve or reject without waiting for sync', async () => {
  const e = setup();
  try {
    const ok = e.store.executeMclCommand('EFFECT wake');
    e.reply(e.sent.at(-1)!.id);
    assert.equal(await ok, 'Unit');
    const failed = e.store.executeMclCommand('BAD');
    e.receive({
      type: 'mcl.execute.result',
      id: e.sent.at(-1)!.id,
      success: false,
      error: { code: 'ParseFailed', message: 'bad command' },
    });
    await assert.rejects(failed, /bad command/);
  } finally {
    e.cleanup();
  }
});
test('MCL config and snapshot query results preserve existing Driver-aware caller shape', async () => {
  const e = setup();
  try {
    for (const [kind, value, expected] of [
      ['config', { sandbox: [] }, { Config: { sandbox: [] } }],
      [
        'block_snapshots',
        { timeline: {}, snapshots: [] },
        { BlockSnapshots: { timeline: {}, snapshots: [] } },
      ],
    ] as const) {
      const p = e.store.executeMclCommand('CONFIG GET');
      e.reply(e.sent.at(-1)!.id, { kind, value });
      assert.deepEqual(await p, expected);
    }
  } finally {
    e.cleanup();
  }
});
test('command results for a removed Agent cannot complete as success', async () => {
  const e = setup();
  try {
    const p = e.store.executeMclCommand('GET session.conversation');
    const id = e.sent.at(-1)!.id;
    e.receive({ type: 'agent.sync', agents: [] });
    e.reply(id, { kind: 'messages', value: [] });
    await assert.rejects(p, /instance changed/);
  } finally {
    e.cleanup();
  }
});
test('Workspace result settles operation but cannot invent runtime state', async () => {
  const e = setup([{ ...ws, address: null, status: 'inactive' }], []);
  try {
    const p = e.store.startWorkspace(path);
    const request = e.sent.at(-1)!;
    assert.equal(request.type, 'workspace.start');
    e.receive({ type: 'workspace.start.result', id: request.id, success: true });
    await p;
    assert.equal(e.store.runtimeWorkspaces.length, 0);
    e.receive({ type: 'workspace.sync', workspaces: [ws] });
    assert.equal(e.store.runtimeWorkspaces[0]?.id, workspace);
  } finally {
    e.cleanup();
  }
});
test('Workspace stop uses definition path and result does not optimistically delete instance', async () => {
  const e = setup();
  try {
    const p = e.store.stopWorkspace();
    const request = e.sent.at(-1)!;
    assert.deepEqual(request.message, { path });
    e.receive({ type: 'workspace.stop.result', id: request.id, success: true });
    await p;
    assert.equal(e.store.runtimeWorkspaces.length, 1);
  } finally {
    e.cleanup();
  }
});
test('chunk accepts shared sequence once for both reasoning and content', () => {
  const e = setup();
  try {
    e.receive(chunk(1, { reasoning: 'think', content: 'answer' }));
    e.receive(chunk(1, { reasoning: 'think', content: 'answer' }));
    assert.equal(e.store.visibleMessages[0]?.thinking, 'think');
    assert.equal(e.store.visibleMessages[0]?.content, 'answer');
    e.receive(chunk(2, { content: ' more' }));
    assert.equal(e.store.visibleMessages[0]?.content, 'answer more');
  } finally {
    e.cleanup();
  }
});
test('full replaces the matching generation and deduplicates persisted messages', () => {
  const e = setup();
  try {
    e.receive(chunk());
    e.receive(full());
    e.receive(full());
    assert.equal(e.store.visibleMessages.length, 1);
    assert.equal(e.store.visibleMessages[0]?.content, 'complete');
    assert.equal(e.store.visibleMessages[0]?.sequence, 42);
    e.receive(chunk(3, { content: 'late' }));
    assert.equal(e.store.visibleMessages.length, 1);
    assert.equal(e.store.visibleMessages[0]?.content, 'complete');
  } finally {
    e.cleanup();
  }
});
test('full without persistence remains a complete render item and has no fake sequence', () => {
  const e = setup();
  try {
    e.receive(chunk());
    e.receive(full(null));
    assert.equal(e.store.visibleMessages.length, 1);
    assert.equal(e.store.visibleMessages[0]?.content, 'complete');
    assert.equal(e.store.visibleMessages[0]?.sequence, null);
  } finally {
    e.cleanup();
  }
});
test('non-generation full appends without overwriting a different streaming message', () => {
  const e = setup();
  try {
    e.receive(chunk());
    e.receive({
      ...full(40, null),
      message: { type: 'user', sequence: 40, content: 'interruption' },
    });
    assert.equal(e.store.visibleMessages.length, 2);
    assert.ok(e.store.visibleMessages.some((x) => x.content === 'part'));
    assert.ok(e.store.visibleMessages.some((x) => x.content === 'interruption'));
  } finally {
    e.cleanup();
  }
});
test('history uses oldest persistent sequence as an exclusive cursor despite live arrivals', () => {
  const e = setup();
  try {
    const initial = e.sent.find((x) => x.type === 'agent.history.req')!;
    assert.equal(initial.message.limit, 50);
    e.receive(page(initial.id, [historyRecord(201, 'oldest'), historyRecord(300, 'newest')]));
    e.receive({ ...full(320, null), message: { type: 'user', sequence: 320, content: 'live' } });
    e.store.loadOlderHistory();
    const older = e.sent.at(-1)!;
    assert.equal(older.type, 'agent.history.req');
    assert.equal(older.message.before_sequence, 201);
    assert.equal(older.message.limit, 50);
    assert.equal('offset' in older.message, false);
    e.receive(page(older.id, [historyRecord(150, 'older'), historyRecord(201, 'oldest')]));
    assert.deepEqual(
      e.store.visibleMessages.map((x) => x.sequence),
      [150, 201, 300, 320],
    );
  } finally {
    e.cleanup();
  }
});
test('late history for a removed Agent is ignored', () => {
  const e = setup();
  try {
    const initial = e.sent.find((x) => x.type === 'agent.history.req')!;
    e.receive({ type: 'agent.sync', agents: [] });
    e.receive(page(initial.id, [historyRecord(201, 'stale')]));
    assert.equal(e.store.visibleMessages.length, 0);
  } finally {
    e.cleanup();
  }
});
test('task snapshots replace the entire collection and retain structured windows', () => {
  const e = setup();
  try {
    const tasks: TaskSnapshot[] = [
      {
        task_id: 'task-1',
        call_id: 'call-1',
        alias: 'bash',
        returned: true,
        output: ['first', { event: 'progress' }],
        error: null,
      },
    ];
    e.receive({ type: 'agent.task.sync', address, tasks });
    assert.deepEqual(e.store.taskStates.get(`${address}`), tasks);
    e.receive({ type: 'agent.task.sync', address, tasks: [] });
    assert.deepEqual(e.store.taskStates.get(`${address}`), []);
  } finally {
    e.cleanup();
  }
});
test('Agent logs and MCL records are separate incremental sets and range requests are explicit', () => {
  const e = setup();
  try {
    e.store.queryRecords('log', { before_sequence: 100, after_sequence: 10, limit: 20 });
    assert.equal(e.sent.at(-1)?.type, 'agent.log.req');
    assert.deepEqual(e.sent.at(-1)?.message, {
      address,

      limit: 20,
      before_sequence: 100,
      after_sequence: 10,
    });
    const row = {
      sequence: 5,
      created_at_ms: 10,
      level: 'info',
      source: 'driver',
      stage: 'execute',
      message: 'hello',
      details_json: null,
      context_json: null,
      origin: null,
      raw_error: null,
      traceback: null,
    };
    e.receive({ type: 'agent.log.sync', address, records: [row] });
    e.receive({ type: 'agent.log.sync', address, records: [row] });
    assert.equal(e.store.agentLogs.get(`${address}`)?.length, 1);
    const audit = {
      sequence: 5,
      created_at_ms: 10,
      source: 'client:webui/test:1',
      turn: null,
      content: 'CONFIG GET',
      payload: { text: 'CONFIG GET' },
    };
    e.receive({ type: 'agent.mcl.sync', address, records: [audit] });
    assert.equal(e.store.mclRecords.get(`${address}`)?.length, 1);
    assert.equal(e.store.visibleMessages.length, 0);
  } finally {
    e.cleanup();
  }
});
test('abort uses original target instance and turn and cannot accept unrelated receipts', async () => {
  const e = setup();
  try {
    e.receive({ type: 'agent.sync', agents: [{ ...agent, status: 'working' }] });
    e.receive({ type: 'agent.turn.sync', address, turn: 'turn-1' });
    e.receive(chunk());
    const p = e.store.abortTurn();
    const wire = e.sent.at(-1)!;
    assert.equal(wire.type, 'agent.turn.abort');
    assert.deepEqual(wire.message, { address, turn: 'turn-1' });
    e.receive({ type: 'agent.turn.abort.result', id: wire.id, success: true });
    await p;
    e.receive(chunk(2, { content: 'late' }));
    assert.equal(e.store.visibleMessages[0]?.content, 'part');
    e.receive({ type: 'agent.turn.sync', address, turn: null });
    assert.equal(e.store.canAbortTurn, false);
  } finally {
    e.cleanup();
  }
});
test('send user input remains explicit MCL injection followed by wake', async () => {
  const e = setup();
  try {
    const p = e.store.sendMessage(' hello ');
    const inject = e.sent.at(-1)!;
    assert.equal(inject.type, 'mcl.execute');
    assert.equal(inject.message.command, 'INJECT ? TO entry.pending[-1]');
    assert.deepEqual(inject.message.arguments, [[{ type: 'user', content: 'hello' }]]);
    e.reply(inject.id);
    await Promise.resolve();
    await Promise.resolve();
    const wake = e.sent.at(-1)!;
    assert.equal(wake.message.command, 'EFFECT wake');
    e.reply(wake.id);
    await p;
  } finally {
    e.cleanup();
  }
});
test('failed injection does not wake and errors preserve the callers draft', async () => {
  const e = setup();
  try {
    const p = e.store.sendMessage('keep');
    e.receive({
      type: 'mcl.execute.result',
      id: e.sent.at(-1)!.id,
      success: false,
      error: { code: 'Rejected', message: 'injection rejected' },
    });
    await assert.rejects(p, /injection rejected/);
    assert.ok(!e.sent.some((x) => x.message.command === 'EFFECT wake'));
  } finally {
    e.cleanup();
  }
});
test('strict message parser rejects replay and images in nested and returned payloads', () => {
  for (const bad of [
    { type: 'assistant', content: 'bad', replay: null },
    { type: 'user', content: 'bad', images: [] },
    { type: 'inject', messages: [{ type: 'user', content: 'bad', images: [] }] },
  ]) {
    assert.equal(isAgentMessage(bad), false);
    assert.equal(
      parseServerMessage(
        JSON.stringify({
          type: 'mcl.execute.result',
          id: 'x',
          success: true,
          data: { kind: 'messages', value: [bad] },
        }),
      ),
      null,
    );
  }
});
test('history and full require a matching real persistence sequence', () => {
  assert.equal(
    parseServerMessage(
      JSON.stringify({ ...full(), message: { type: 'assistant', sequence: 43, content: 'bad' } }),
    ),
    null,
  );
  assert.equal(
    parseServerMessage(
      JSON.stringify(
        page('x', [{ ...historyRecord(5, 'bad'), payload: { type: 'user', content: 'bad' } }]),
      ),
    ),
    null,
  );
  assert.ok(parseServerMessage(JSON.stringify(full())));
});
test('failed query cannot disguise failure as an empty page', () => {
  assert.equal(
    parseServerMessage(
      JSON.stringify({
        type: 'agent.history.res',
        id: 'x',
        address,

        success: false,
        error: { code: 'ReadFailed', message: 'failed' },
        records: [],
        next_before_sequence: null,
        next_after_sequence: null,
      }),
    ),
    null,
  );
});
test('registration result is matched to the current connection request', () => {
  const e = setup();
  try {
    e.receive({
      type: 'connection.register.result',
      id: 'unknown',
      success: true,
      client: { address: 'client:wrong/x:1', client_type: 'webui' },
    });
    assert.equal(e.store.registeredClient?.address, 'client:webui/test:1');
    const registration = e.sent[0]!;
    assert.equal(registration.type, 'connection.register');
    assert.match(
      String((registration.message as { name?: string }).name),
      /^webui-[0-9a-z-]+$/,
    );
  } finally {
    e.cleanup();
  }
});
test('context selectors and JSON argument parsing do not create command injection', () => {
  assert.deepEqual(parseArguments('["alias", {"x":1}, null]'), ['alias', { x: 1 }, null]);
  assert.throws(() => parseArguments('{}'), /array/);
  const image = { media_type: 'image/png', data: 'YWJj' };
  assert.deepEqual(contextInjection('', 'entry.pending[-1]', [image]).arguments, [
    [{ type: 'user', content: '', data: [image] }],
  ]);
  for (const selector of ['', 'field', 'x.y;EFFECT wake', 'x\nEFFECT wake'])
    assert.throws(() => contextInjection('hi', selector));
});
test('media and tool formatting retain structured data', () => {
  const image = { media_type: 'image/png', data: 'YWJj', local: 'not wire' };
  assert.deepEqual(wireImages([image]), [{ media_type: 'image/png', data: 'YWJj' }]);
  assert.throws(() => validateImageBudget(Array(9).fill(1)), /8/);
  assert.ok(formatToolPayload({ hello: 'world' }).includes('hello'));
});
test('history helper renders data and usage without exposing metadata as text', () => {
  const entry: AgentHistory = {
    workspace: { id: workspace, name: 'Demo', project_root: '/project' },
    agent: address,
    messages: [
      {
        sequence: 7,
        turn: null,
        kind: 'message',
        content: '',
        payload: {
          type: 'assistant',
          sequence: 7,
          content: 'answer',
          reasoning: 'thought',
          usage: { input_tokens: 100, output_tokens: 10 },
          tool_calls: [],
        },
        created_at_ms: 10,
      },
    ],
  };
  assert.equal(historyEntries(entry)[0]?.content, 'answer');
  assert.equal(historyEntries(entry)[0]?.thinking, 'thought');
  assert.equal(latestContextTokens(entry), 110);
});

function history(messages: AgentHistory['messages']): AgentHistory {
  return {
    workspace: { id: workspace, name: 'Demo', project_root: '/project' },
    agent: address,
    messages,
  };
}
const state = {
  workspace_definitions: [{ path, cache: ws.definition }],
  workspaces: [
    {
      id: workspace,
      name: 'Demo',
      project_root: '/project',
      definition_path: path,
      status: 'running' as const,
      agents: [address],
    },
  ],
  agents: [
    {
      workspace: { id: workspace, name: 'Demo', project_root: '/project' },
      agent: address,

      status: 'idle' as const,
      turn: null,
      sandbox: null,
      tools: ['read_file'],
      sandboxes: [],
      queued_commands: 0,
      pending_command: null,
    },
  ],
  histories: [],
};

test('history keeps unknown records and background task identifiers as plain text and metadata', () => {
  const entry = historyEntries(
    history([
      {
        source: 'agent:demo/coder:latest',
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

test('images keep wire-only metadata and reject unsupported formats or excessive budgets', async () => {
  const image = { media_type: 'image/png', data: 'iVBORw0KGgo=', name: 'private.png', size: 8 };
  assert.deepEqual(wireImages([image]), [{ media_type: image.media_type, data: image.data }]);
  assert.ok(!metadataText({ data: [image] }).includes(image.data));
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
      data: [{ media_type: 'image/svg+xml', data: 'abcd' }],
    }),
    false,
  );
});

test('sidebar status uses runtime state and errors while preserving configured path identity', () => {
  const model = buildSidebarModel(
    state.workspace_definitions,
    state.workspaces,
    state.agents,
    new Map(),
    new Set([path]),
    path,
    address,
  );
  assert.equal(model.workspaces[0]!.key, path);
  assert.equal(model.workspaces[0]!.address, workspace);
  assert.equal(model.workspaces[0]!.status, 'running');
  const failed = buildSidebarModel(
    state.workspace_definitions,
    state.workspaces,
    [{ ...state.agents[0]!, error: { code: 'Failure', message: 'actual failure' } }],
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
        onStartWorkspace: (...args: unknown[]) => events.push(args),
        onRestartWorkspace: (...args: unknown[]) => events.push(args),
        onStopWorkspace: (...args: unknown[]) => events.push(args),
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
    assert.deepEqual(events, [[address, path]]);
    // A running workspace exposes restart + stop, not start.
    const actions = nodes.filter(
      (node) => node.type === 'button' && String(node.props.class).includes('tree-action'),
    );
    assert.deepEqual(
      actions.map((action) => action.props.title),
      ['Restart workspace', 'Stop workspace'],
    );
    const event = { stopPropagation() {} };
    (actions[0]!.props.onClick as (value: unknown) => void)(event);
    (actions[1]!.props.onClick as (value: unknown) => void)(event);
    assert.deepEqual(events.slice(1), [[path], [workspace]]);
    const text = (node: Host): string => node.text + node.children.map(text).join('');
    assert.ok(!/New workspace|New session|Rename|Delete|Refresh/.test(text(root)));
  } finally {
    app.unmount();
  }
});

test('assistant messages may omit content, reasoning and tool_calls', () => {
  assert.equal(isAgentMessage({ type: 'assistant' }), true);
  assert.equal(
    isAgentMessage({
      type: 'assistant',
      tool_calls: [{ id: 'call', tool_name: 'read_file', arguments: '{}' }],
    }),
    true,
  );
  const entries = historyEntries(
    history([
      {
        type: 'assistant',
        sequence: 5,
        turn: null,
        kind: 'message',
        content: '',
        payload: {
          type: 'assistant',
          tool_calls: [{ id: 'call', tool_name: 'read_file', arguments: '{}' }],
        },
        created_at_ms: 1,
      },
    ]),
  );
  assert.equal(entries[0]!.role, 'assistant');
  assert.equal(entries[0]!.content, '');
  assert.equal(entries[0]!.thinking, '');
  assert.equal(entries[0]!.toolCalls.length, 1);
});

test('tool results merge into the assistant call row', () => {
  const entries = historyEntries(
    history([
      {
        type: 'assistant',
        turn: null,
        sequence: 1,
        kind: 'message',
        content: '',
        payload: {
          type: 'assistant',
          tool_calls: [
            { id: 'call-a', tool_name: 'read_file', arguments: '{"path":"a"}' },
            { id: 'call-b', tool_name: 'grep', arguments: '{"q":"b"}' },
          ],
        },
        created_at_ms: 1,
      },
      {
        type: 'tool_call',
        turn: null,
        sequence: 2,
        kind: 'message',
        content: 'ok',
        payload: {
          type: 'tool_call',
          tool_call_id: 'call-a',
          tool_name: 'read_file',
          content: 'ok',
          failed: false,
        },
        created_at_ms: 2,
      },
      {
        type: 'tool_call',
        turn: null,
        sequence: 3,
        kind: 'message',
        content: 'failed',
        payload: {
          type: 'tool_call',
          tool_call_id: 'call-b',
          tool_name: 'grep',
          content: 'failed',
          failed: true,
        },
        created_at_ms: 3,
      },
    ]),
  );
  assert.equal(entries.length, 1);
  const call = entries[0]!;
  assert.equal(call.role, 'assistant');
  assert.deepEqual(
    call.toolCalls.map((tool) => tool.id),
    ['call-a', 'call-b'],
  );
  assert.deepEqual(call.toolResults?.get('call-a'), {
    content: 'ok',
    failed: false,
    images: undefined,
  });
  assert.deepEqual(call.toolResults?.get('call-b'), {
    content: 'failed',
    failed: true,
    images: undefined,
  });
});

test('an unpaired tool result stays as a standalone tool row', () => {
  const entries = historyEntries(
    history([
      {
        type: 'tool_call',
        turn: null,
        sequence: 1,
        kind: 'message',
        content: 'orphan',
        payload: {
          type: 'tool_call',
          tool_call_id: 'call-orphan',
          tool_name: 'read_file',
          content: 'orphan',
          failed: true,
        },
        created_at_ms: 1,
      },
    ]),
  );
  assert.equal(entries.length, 1);
  assert.equal(entries[0]!.role, 'tool');
  assert.equal(entries[0]!.toolCallId, 'call-orphan');
  assert.equal(entries[0]!.toolName, 'read_file');
  assert.equal(entries[0]!.failed, true);
});

test('tool payloads render as readable TOML-like text', () => {
  const value = { cmd: 'time sleep 9; echo "9 秒结束"', nested: { retry: 2 } };
  const text = formatToolPayload(value);
  assert.ok(text.includes(`cmd = 'time sleep 9; echo "9 秒结束"'`));
  assert.ok(text.includes('[nested]'));
  assert.ok(text.includes('retry = 2'));
  assert.equal(formatToolPayload(JSON.stringify(value)), text);
  assert.equal(inlineToolPayload(value).includes('\n'), false);
  assert.equal(formatToolPayload('hello world'), 'hello world');
  const result = formatToolPayload(JSON.stringify({ output: ['a\nb'], failed: false }));
  assert.ok(result.includes('output = ['));
  assert.ok(result.includes("'''a\nb'''"));
  assert.ok(result.includes('failed = false'));
});

test('multiple activated Agents require an explicit choice and route changes never wake another Agent', async () => {
  const other = { ...agent, address: 'agent:demo/reviewer:latest' };
  const e = setup(
    [
      {
        ...ws,
        definition: {
          ...ws.definition,
          agents: [
            ...ws.definition.agents,
            { name: 'reviewer', driver: 'driver:local/reviewer:latest', home_root: '/reviewer' },
          ],
        },
      },
    ],
    [agent, other],
  );
  try {
    assert.equal(e.store.selectedAgent, null);
    await assert.rejects(e.store.executeMclCommand('GET session.conversation'), /explicitly/);
    e.store.selectAgent(address);
    const sending = e.store.sendMessage('keep draft');
    const inject = e.sent.at(-1)!;
    e.store.selectAgent(other.address);
    e.reply(inject.id);
    await assert.rejects(sending, /changed during send/);
    assert.ok(!e.sent.some((w) => w.message.command === 'EFFECT wake'));
  } finally {
    e.cleanup();
  }
});
test('failed wake retries only the wake, without duplicate input injection', async () => {
  const e = setup();
  try {
    const first = e.store.sendMessage('same');
    e.reply(e.sent.at(-1)!.id);
    await Promise.resolve();
    await Promise.resolve();
    const wake = e.sent.at(-1)!;
    assert.equal(wake.message.command, 'EFFECT wake');
    e.receive({
      type: 'mcl.execute.result',
      id: wake.id,
      success: false,
      error: { code: 'Rejected', message: 'wake rejected' },
    });
    await assert.rejects(first, /wake rejected/);
    const retry = e.store.sendMessage('same');
    assert.equal(e.sent.at(-1)!.message.command, 'EFFECT wake');
    e.reply(e.sent.at(-1)!.id);
    await retry;
    assert.equal(
      e.sent.filter((w) => w.message.command === 'INJECT ? TO entry.pending[-1]').length,
      1,
    );
  } finally {
    e.cleanup();
  }
});
test('a non-Unit injection value is an unknown outcome and never wakes the Driver', async () => {
  const e = setup();
  try {
    const p = e.store.sendMessage('draft');
    e.reply(e.sent.at(-1)!.id, { kind: 'config', value: {} });
    await assert.rejects(p, /did not acknowledge Unit/);
    assert.ok(!e.sent.some((w) => w.message.command === 'EFFECT wake'));
  } finally {
    e.cleanup();
  }
});
test('multi-MiB media sends await receipts and diagnostics do not contain media bytes', async () => {
  const e = setup();
  try {
    const image = { media_type: 'image/png', data: 'A'.repeat(2 * 1024 * 1024) };
    const p = e.store.sendMessage('', [image]);
    const inject = e.sent.at(-1)!;
    assert.ok(JSON.stringify(inject).length > 1024 * 1024);
    assert.deepEqual(inject.message.arguments, [[{ type: 'user', content: '', data: [image] }]]);
    e.reply(inject.id);
    await Promise.resolve();
    await Promise.resolve();
    e.reply(e.sent.at(-1)!.id);
    await p;
    assert.ok(!JSON.stringify(e.store.logs).includes(image.data));
  } finally {
    e.cleanup();
  }
});
test('abort failure stays frozen until current matching turn sync releases it', async () => {
  const e = setup();
  try {
    e.receive({ type: 'agent.sync', agents: [{ ...agent, status: 'working' }] });
    e.receive({ type: 'agent.turn.sync', address, turn: 'turn-1' });
    e.receive(chunk());
    const p = e.store.abortTurn();
    const abort = e.sent.at(-1)!;
    e.receive({
      type: 'agent.turn.abort.result',
      id: abort.id,
      success: false,
      error: { code: 'Rejected', message: 'abort rejected' },
    });
    await assert.rejects(p, /abort rejected/);
    e.receive(chunk(2, { content: 'blocked' }));
    assert.equal(e.store.visibleMessages[0]?.content, 'part');
    e.receive({ type: 'agent.turn.sync', address, turn: 'turn-1' });
    e.receive(chunk(3, { content: ' resumed' }));
    assert.equal(e.store.visibleMessages[0]?.content, 'part resumed');
  } finally {
    e.cleanup();
  }
});
test('late abort receipt completes original Promise without mutating new VM turn', async () => {
  const e = setup();
  try {
    e.receive({ type: 'agent.sync', agents: [{ ...agent, status: 'working' }] });
    e.receive({ type: 'agent.turn.sync', address, turn: 'turn-1' });
    const p = e.store.abortTurn();
    const abort = e.sent.at(-1)!;
    e.receive({ type: 'agent.sync', agents: [{ ...agent, status: 'working' }] });
    e.receive({ type: 'agent.turn.sync', address, turn: 'turn-new' });
    e.receive({ type: 'agent.turn.abort.result', id: abort.id, success: true });
    await p;
    assert.equal(e.store.selectedAgentState?.turn, 'turn-new');
  } finally {
    e.cleanup();
  }
});
test('history and full reconcile streams by explicit generation association, never equal text', () => {
  const e = setup();
  try {
    e.receive(chunk());
    const first = e.sent.find((w) => w.type === 'agent.history.req')!;
    e.receive(
      page(first.id, [
        {
          ...historyRecord(79, 'part'),
          payload: {
            type: 'assistant',
            sequence: 79,
            request_id: 'other-generation',
            content: 'part',
          },
        },
      ]),
    );
    assert.equal(e.store.visibleMessages.length, 2);
    e.receive({
      ...full(80, 'generation-1', 'part'),
      message: { type: 'assistant', sequence: 80, request_id: 'generation-1', content: 'part' },
    });
    assert.equal(e.store.visibleMessages.length, 2);
    assert.deepEqual(
      e.store.visibleMessages.map((v) => v.sequence),
      [79, 80],
    );
  } finally {
    e.cleanup();
  }
});
test('history token totals use persisted assistant usage and latest sequence', () => {
  const e = setup();
  try {
    const id = e.sent.find((w) => w.type === 'agent.history.req')!.id;
    const rows = [
      {
        ...historyRecord(42, 'answer'),
        payload: {
          type: 'assistant',
          sequence: 42,
          content: 'answer',
          usage: { input_tokens: 60, output_tokens: 4, cache_hit_tokens: 3 },
        },
      },
      {
        ...historyRecord(41, 'answer'),
        payload: {
          type: 'assistant',
          sequence: 41,
          content: 'answer',
          usage: { input_tokens: 10, output_tokens: 2, cache_hit_tokens: 3 },
        },
      },
    ];
    e.receive(page(id, rows));
    assert.deepEqual(e.store.tokenUsage, { input: 70, output: 6, cached: 6 });
    assert.equal(e.store.contextTokens, 64);
    assert.equal(e.store.contextWindowTokens, 131072);
  } finally {
    e.cleanup();
  }
});
test('sandbox choices and model come from the authoritative agent snapshot', async () => {
  const e = setup();
  try {
    assert.deepEqual(e.store.sandboxOptions.options, ['workspace_write', 'read_only']);
    assert.deepEqual(e.store.sandboxOptions.activated, ['workspace_write']);
    assert.equal(e.store.contextWindowTokens, 131072);

    const applying = e.store.setSandbox('read_only');
    const set = e.sent.at(-1)!;
    assert.equal(set.message.command, 'CONFIG SET ? ?');
    assert.deepEqual(set.message.arguments, ['sandbox', ['read_only']]);
    e.reply(set.id);
    await new Promise<void>((resolve) => setImmediate(resolve));
    const effect = e.sent.at(-1)!;
    assert.equal(effect.message.command, 'EFFECT sandbox_use ?');
    e.reply(effect.id);
    await applying;
    e.receive({ type: 'agent.sync', agents: [{ ...agent, sandbox: 'read_only' }] });
    assert.deepEqual(e.store.sandboxOptions.activated, ['read_only']);
  } finally {
    e.cleanup();
  }
});
test('connecting again clears state readiness until new category snapshots arrive', () => {
  const e = setup();
  try {
    assert.equal(e.store.stateReady, true);
    e.store.connect();
    assert.equal(e.store.stateReady, false);
  } finally {
    e.cleanup();
  }
});
test('a completed send can submit identical content again after a later turn', async () => {
  const e = setup();
  try {
    for (let i = 0; i < 2; i++) {
      const p = e.store.sendMessage('repeat');
      e.reply(e.sent.at(-1)!.id);
      await Promise.resolve();
      await Promise.resolve();
      e.reply(e.sent.at(-1)!.id);
      await p;
    }
    assert.equal(
      e.sent.filter((w) => w.message.command === 'INJECT ? TO entry.pending[-1]').length,
      2,
    );
  } finally {
    e.cleanup();
  }
});
