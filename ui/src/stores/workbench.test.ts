/// <reference types="node" />
import { strict as assert } from 'node:assert';
import { test } from 'node:test';
import { createPinia, setActivePinia } from 'pinia';
import { contextInjection, parseArguments, historyEntries, useWorkbenchStore } from './workbench';
import {
  isAgentMessage,
  parseServerMessage,
  type AgentHistory,
  type BackendState,
} from '../types/protocol';
const workspace = {
  id: 'workspace:local/test:latest',
  name: 'test',
  project_root: '/test',
  manager: 'agent:local/manager:latest',
  agents: ['agent:local/manager:latest'],
};
const state: BackendState = {
  workspaces: [workspace],
  agents: [
    {
      workspace,
      agent: workspace.manager,
      agent_instance: 'Entity(1, 2)',
      status: 'running',
      turn: null,
      sandbox: null,
      queued_commands: 0,
      pending_command: null,
    },
  ],
  histories: [],
};
test('explicit driver convention and array placeholder arguments', () => {
  assert.throws(() => contextInjection('hello', ''));
  assert.throws(() => contextInjection('hello', 'x\nEMIT wake'));
  assert.deepEqual(contextInjection('hello', 'driver.messages'), {
    command: 'INJECT ? TO driver.messages',
    arguments: [[{ type: 'user', content: 'hello' }]],
  });
  assert.deepEqual(parseArguments('[[1,2],[3]]'), [[1, 2], [3]]);
  assert.throws(() => parseArguments('{"binding":1}'));
  assert.throws(() => parseArguments('[1]'));
});
test('protocol accepts new structured error and terminal, rejects old transport messages', () => {
  assert.equal(parseServerMessage(JSON.stringify({ type: 'agent.turn.abort', id: 'old' })), null);
  assert.equal(
    parseServerMessage(
      JSON.stringify({ type: 'agent.message', message: { User: { content: 'old' } } }),
    ),
    null,
  );
  assert.equal(
    parseServerMessage(
      JSON.stringify({
        type: 'agent.message.delta',
        id: 'x',
        agent: workspace.manager,
        content: 'old',
      }),
    ),
    null,
  );
  assert.equal(
    parseServerMessage(
      JSON.stringify({
        type: 'agent.message.finished',
        id: 'x',
        agent: workspace.manager,
        agent_instance: 'Entity(1, 2)',
        error: null,
      }),
    )?.type,
    'agent.message.finished',
  );
  const opened = {
    BlockOpened: { block_id: 'Case-sensitive.block', created: false, temporary: true },
  };
  const response = parseServerMessage(
    JSON.stringify({
      type: 'mcl.command_result',
      id: 'block',
      source: 'client:local/a',
      target: workspace.manager,
      result: { Ok: opened },
      history_error: null,
    }),
  );
  assert.ok(response?.type === 'mcl.command_result');
  assert.deepEqual(response.result, { Ok: opened });
  assert.equal(
    parseServerMessage(
      JSON.stringify({
        type: 'mcl.command_result',
        id: 'x',
        source: 'client:local/a',
        target: workspace.manager,
        result: { Err: { kind: 'MclError', message: 'failure' } },
        history_error: null,
      }),
    )?.type,
    'mcl.command_result',
  );
});
test('history recursively projects lower-case messages and retains record payload', () => {
  const history: AgentHistory = {
    workspace,
    agent: workspace.manager,
    messages: [
      {
        key: 'one',
        sequence: 1,
        turn: null,
        kind: 'message',
        content: '',
        created_at_ms: 0,
        payload: {
          type: 'inject',
          messages: [
            { type: 'user', content: 'hello' },
            {
              type: 'tool_call',
              resource_id: null,
              tool_call_id: 'call',
              tool_name: 'missing',
              content: 'not loaded',
              failed: true,
            },
          ],
        },
      },
      {
        key: 'two',
        sequence: 2,
        turn: null,
        kind: 'mcl',
        content: 'INJECT ? TO x',
        created_at_ms: 0,
        payload: { text: 'INJECT ? TO x', arguments: [[1]] },
      },
    ],
  };
  const entries = historyEntries(history);
  assert.deepEqual(
    entries.map((e) => e.role),
    ['user', 'tool', 'record'],
  );
  assert.equal(entries[1]?.failed, true);
  assert.equal(entries[0]?.timestamp, 0);
  assert.deepEqual(entries[2]?.payload, { text: 'INJECT ? TO x', arguments: [[1]] });
});
test('system context messages retain their role through recursive history projection', () => {
  const message = {
    type: 'inject',
    messages: [{ type: 'inject', messages: [{ type: 'system', content: 'real system prompt' }] }],
  };
  assert.equal(isAgentMessage(message), true);
  assert.equal(isAgentMessage({ type: 'system', content: 1 }), false);
  const history: AgentHistory = {
    workspace,
    agent: workspace.manager,
    messages: [
      {
        key: 'system',
        sequence: 1,
        turn: null,
        kind: 'message',
        content: '',
        created_at_ms: 0,
        payload: { message },
      },
    ],
  };
  const entries = historyEntries(history);
  assert.equal(entries.length, 1);
  assert.equal(entries[0]?.role, 'system');
  assert.equal(entries[0]?.content, 'real system prompt');
  assert.deepEqual(entries[0]?.payload, { message });
});
test('streams reject stale instances and duplicate sequence, terminal blocks late deltas', () => {
  setActivePinia(createPinia());
  const store = useWorkbenchStore();
  store.handleServerMessage({ type: 'state.sync', state });
  const delta = {
    type: 'agent.message.delta' as const,
    id: 'inference',
    agent: workspace.manager,
    agent_instance: 'Entity(1, 2)',
    sequence: 1,
    content: 'hello',
  };
  store.handleServerMessage({ ...delta, agent_instance: 'Entity(1, 1)' });
  assert.equal(store.visibleMessages.length, 0);
  store.handleServerMessage(delta);
  store.handleServerMessage(delta);
  assert.equal(store.visibleMessages[0]?.content, 'hello');
  store.handleServerMessage({
    type: 'agent.message.finished',
    id: delta.id,
    agent: delta.agent,
    agent_instance: delta.agent_instance,
    error: null,
  });
  store.handleServerMessage({
    type: 'history.result',
    id: 'history',
    history: { workspace, agent: workspace.manager, messages: [] },
  });
  store.handleServerMessage({ ...delta, sequence: 2, content: 'late' });
  assert.equal(store.visibleMessages.length, 0);
});
test('compatibility failures and abort remain truthful without legacy protocol', async () => {
  setActivePinia(createPinia());
  const store = useWorkbenchStore();
  store.handleServerMessage({ type: 'state.sync', state });
  assert.equal(store.driverContextPath, 'entry.pending[-1]');
  assert.deepEqual(store.resourceVisibility, []);
  assert.deepEqual(store.sandboxOptions, { options: [], activated: [] });
  assert.equal(store.canAbortTurn, false);
  await assert.rejects(store.abortTurn(), /not supported/);
  await assert.rejects(store.sendSkillInvocation('', 'hook:local/test'), /Hook execution/);
  await assert.rejects(
    store.setResourceVisibility('tool:local/unknown', true),
    /verified current TOOL alias/,
  );
  await assert.rejects(store.setSandbox('sandbox:local/unknown'), /verified current SANDBOX alias/);
  store.handleServerMessage({
    type: 'state.sync',
    state: {
      ...state,
      agents: [{ ...state.agents[0]!, turn: 'active' }],
    },
  });
  assert.equal(store.selectedAgentWorking, true);
  store.handleServerMessage({
    type: 'agent.message.finished',
    id: 'failed',
    agent: workspace.manager,
    agent_instance: 'Entity(1, 2)',
    error: { kind: 'InferenceFailed', message: 'real failure' },
  });
  assert.deepEqual(store.lastFailures.get(`${workspace.id}:${workspace.manager}`), {
    message: 'real failure',
  });
  store.handleServerMessage({
    type: 'agent.message.finished',
    id: 'stale',
    agent: workspace.manager,
    agent_instance: 'old',
    error: { kind: 'InferenceFailed', message: 'stale failure' },
  });
  assert.equal(
    store.lastFailures.get(`${workspace.id}:${workspace.manager}`)?.message,
    'real failure',
  );
});

test('current typed GET restores resource controls and composer uses array arguments plus wake', async () => {
  setActivePinia(createPinia());
  const store = useWorkbenchStore();
  const sent: Array<{
    type: string;
    id: string;
    message: { command?: string; arguments?: unknown[][] };
  }> = [];
  const review = {
    resource_id: 'skill:local/review',
    alias: 'review',
    description: '',
    input_schema: {},
  };
  const sandbox = { resource_id: 'sandbox:local/write' };
  let visible = [review];
  let activated = [sandbox];
  const oldSocket = globalThis.WebSocket;
  class Socket {
    static OPEN = 1;
    readyState = 1;
    onopen: (() => void) | null = null;
    onclose: (() => void) | null = null;
    onmessage = null;
    onerror = null;
    close() {}
    send(raw: string) {
      const request = JSON.parse(raw) as (typeof sent)[number];
      sent.push(request);
      if (request.type !== 'mcl.command') return;
      const command = request.message.command!;
      let result: unknown = 'Unit';
      if (command === 'GET review') result = { Array: { Tool: [review] } };
      if (command === 'GET write') result = { Array: { Sandbox: [sandbox] } };
      if (command === 'GET session.visible') result = { Array: { Tool: visible } };
      if (command === 'GET session.sandbox_activated') result = { Array: { Sandbox: activated } };
      if (command === 'INJECT ? TO session.visible')
        visible = request.message.arguments?.[0]?.length ? [review] : [];
      if (command === 'INJECT ? TO session.sandbox_activated')
        activated = request.message.arguments?.[0]?.length ? [sandbox] : [];
      queueMicrotask(() =>
        store.handleServerMessage({
          type: 'mcl.command_result',
          id: request.id,
          source: 'client:test',
          target: workspace.manager,
          result: { Ok: result } as never,
          history_error: null,
        }),
      );
    }
  }
  globalThis.WebSocket = Socket as unknown as typeof WebSocket;
  try {
    store.handleServerMessage({
      type: 'state.sync',
      state: {
        ...state,
        histories: [
          {
            workspace,
            agent: workspace.manager,
            messages: [
              'IMPORT TOOL skill:local/review AS review',
              'IMPORT SANDBOX sandbox:local/write AS write',
            ].map((text, index) => ({
              key: String(index),
              sequence: index,
              turn: null,
              kind: 'mcl',
              content: text,
              payload: { text, arguments: [] },
              created_at_ms: 0,
            })),
          },
        ],
      },
    });
    store.connect();
    store.handleServerMessage({
      type: 'connection.registered',
      id: 'register',
      client: { resource_id: 'client:test', name: 'test', client_type: 'webui' },
    });
    await store.refreshDriverResources(true);
    assert.deepEqual(store.resourceVisibility, [
      { resource: review.resource_id, visible: true, default: false },
    ]);
    assert.deepEqual(store.sandboxOptions, {
      options: [sandbox.resource_id],
      activated: [sandbox.resource_id],
    });
    await store.setResourceVisibility(review.resource_id, false);
    assert.equal(store.resourceVisibility[0]?.visible, false);
    await store.setSandbox(null);
    assert.deepEqual(store.sandboxOptions.activated, []);
    await store.sendMessage(' hello ');
    const commands = sent.filter((request) => request.type === 'mcl.command');
    assert.deepEqual(commands.at(-2)?.message, {
      workspace: { id: workspace.id, name: workspace.name, project_root: workspace.project_root },
      agent: null,
      command: 'INJECT ? TO entry.pending[-1]',
      arguments: [[{ type: 'user', content: 'hello' }]],
    });
    assert.equal(commands.at(-1)?.message.command, 'EMIT EFFECT wake');
    assert.ok(sent.every((request) => !request.type.startsWith('agent.')));
  } finally {
    store.disconnect();
    globalThis.WebSocket = oldSocket;
  }
});

test('ended Driver failure remains visible in state snapshots', () => {
  setActivePinia(createPinia());
  const store = useWorkbenchStore();
  store.handleServerMessage({
    type: 'state.sync',
    state: {
      ...state,
      agents: state.agents.map((agent) => ({
        ...agent,
        status: 'stopping' as const,
        turn: null,
        error: { kind: 'DriverExecutionFailed', message: 'tool calls lack results' },
      })),
    },
  });
  assert.equal(store.selectedAgentReady, false);
  assert.equal(store.selectedAgentWorking, false);
  assert.equal(
    store.lastFailures.get(`${workspace.id}:${workspace.manager}`)?.message,
    'tool calls lack results',
  );
});

test('usage comes from persisted history, not state token counters', () => {
  setActivePinia(createPinia());
  const store = useWorkbenchStore();
  store.handleServerMessage({
    type: 'state.sync',
    state: {
      ...state,
      histories: [
        {
          workspace,
          agent: workspace.manager,
          messages: [
            {
              key: 'usage',
              sequence: 1,
              turn: null,
              kind: 'message',
              content: '',
              created_at_ms: 0,
              payload: {
                type: 'assistant',
                reasoning: null,
                content: 'done',
                tool_calls: [],
                usage: { input_tokens: 10, output_tokens: 5, cache_hit_tokens: 3 },
              },
            },
          ],
        },
      ],
    },
  });
  assert.deepEqual(store.tokenUsage, { input: 10, output: 5, cached: 3 });
});
