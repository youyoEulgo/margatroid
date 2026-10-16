/// <reference types="node" />
import { strict as assert } from 'node:assert';
import { test } from 'node:test';
import { createPinia, setActivePinia } from 'pinia';
import {
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
  type BackendState,
} from '../types/protocol';
import { formatRecordTime } from '../lib/time';
import { imageMediaType, validateImageBudget, metadataText, wireImages, importImage } from '../lib/images';

test('history time renders real timestamps and never formats unknown time as epoch', () => {
  for (const timestamp of [0, -1, Number.NaN, Number.POSITIVE_INFINITY, 9e20])
    assert.equal(formatRecordTime(timestamp), 'Time unavailable');
  const timestamp = Date.UTC(2026, 9, 15, 4, 21, 43);
  for (const seconds of [false, true]) {
    assert.equal(
      formatRecordTime(timestamp, seconds),
      new Intl.DateTimeFormat(undefined, {
        hour: '2-digit',
        minute: '2-digit',
        ...(seconds ? { second: '2-digit' as const } : {}),
      }).format(timestamp),
    );
  }
});

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
      status: 'idle',
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
  await assert.rejects(store.abortTurn(), /No abortable turn/);
  assert.equal('sendSkillInvocation' in store, false);
  await assert.rejects(
    store.setResourceVisibility('tool:local/unknown', true),
    /verified current TOOL alias/,
  );
  await assert.rejects(store.setSandbox('sandbox:local/unknown'), /verified current SANDBOX alias/);
  store.handleServerMessage({
    type: 'state.sync',
    state: {
      ...state,
      agents: [{ ...state.agents[0]!, status: 'working', turn: 'active' }],
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
    resource_id: 'tool:local/review',
    alias: 'review',
    description: 'Review a change with an explicit target',
    input_schema: {
      type: 'object',
      properties: { target: { type: 'string' } },
      required: ['target'],
      additionalProperties: false,
    },
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
              'IMPORT TOOL tool:local/review AS review',
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
      agent: workspace.manager,
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

test('split message records reconstruct text and use only latest assistant context usage', () => {
  setActivePinia(createPinia());
  const store = useWorkbenchStore();
  const row = (key: string, content: string, payload: unknown) => ({
    key,
    sequence: 1,
    turn: null,
    kind: 'message',
    content,
    payload,
    created_at_ms: 123,
  });
  const history: AgentHistory = {
    workspace,
    agent: workspace.manager,
    messages: [
      row('user', 'actual user text', { type: 'user' }),
      row('old', 'legacy assistant', {
        type: 'assistant',
        content: 'legacy text',
        reasoning: null,
        tool_calls: [],
        usage: { input_tokens: 100, output_tokens: 20, cache_hit_tokens: 50 },
      }),
      row('new', 'new assistant text', {
        type: 'assistant',
        reasoning: 'think',
        tool_calls: [],
        usage: { input_tokens: 600, output_tokens: 40, cache_hit_tokens: 200 },
      }),
      row('tool', 'actual tool output', {
        type: 'tool_call',
        resource_id: null,
        tool_call_id: 'call',
        tool_name: 'test',
        failed: false,
      }),
      row('error', 'actual error', { type: 'error' }),
      row('inject', '', { type: 'inject', messages: [{ type: 'user', content: 'nested user' }] }),
    ],
  };
  assert.deepEqual(
    historyEntries(history).map((entry) => entry.content),
    [
      'actual user text',
      'legacy text',
      'new assistant text',
      'actual tool output',
      'actual error',
      'nested user',
    ],
  );
  assert.equal(latestContextTokens(history), 640);
  store.handleServerMessage({
    type: 'state.sync',
    state: {
      ...state,
      agents: state.agents.map((agent) => ({ ...agent, context_window_tokens: 1000 })),
      histories: [history],
    },
  });
  assert.equal(store.contextTokens, 640);
  assert.equal(store.contextWindowTokens, 1000);
  assert.deepEqual(store.tokenUsage, { input: 700, output: 60, cached: 250 });
  history.messages.push(
    row('unknown', 'no usage', { type: 'assistant', reasoning: null, tool_calls: [], usage: null }),
  );
  assert.equal(latestContextTokens(history), null);
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

test('composer gates on status not turn or generic MCL pending', async () => {
  setActivePinia(createPinia());
  const store = useWorkbenchStore();
  const sync = (
    status: 'idle' | 'working' | 'creating' | 'stopping',
    turn: string | null,
    pending_command: string | null,
  ) =>
    store.handleServerMessage({
      type: 'state.sync',
      state: { ...state, agents: [{ ...state.agents[0]!, status, turn, pending_command }] },
    });
  sync('idle', 'stale', 'admin-command');
  assert.equal(store.selectedAgentIdle, true);
  assert.equal(store.selectedAgentWorking, false);
  assert.equal(store.canAbortTurn, false);
  sync('working', 'turn-1', null);
  assert.equal(store.selectedAgentIdle, false);
  assert.equal(store.canAbortTurn, true);
  await assert.rejects(store.sendMessage('no overlap'), /not idle/);
  sync('working', null, null);
  assert.equal(store.selectedAgentWorking, true);
  assert.equal(store.abortPending, true);
  assert.equal(store.canAbortTurn, false);
  for (const status of ['creating', 'stopping'] as const) {
    sync(status, null, null);
    assert.equal(store.selectedAgentIdle, false);
    assert.equal(store.selectedAgentWorking, false);
  }
});

test('cancelled assistant history preserves split metadata without content inference', () => {
  const history: AgentHistory = {
    workspace,
    agent: workspace.manager,
    messages: [
      {
        key: 'cancel',
        sequence: 1,
        turn: 't',
        kind: 'message',
        content: 'other text',
        payload: {
          type: 'assistant',
          reasoning: null,
          tool_calls: [],
          usage: null,
          stop_reason: 'cancelled',
        },
        created_at_ms: 123,
      },
      {
        key: 'ordinary',
        sequence: 2,
        turn: 't',
        kind: 'message',
        content: 'Turn aborted.',
        payload: { type: 'assistant', reasoning: null, tool_calls: [] },
        created_at_ms: 124,
      },
    ],
  };
  const entries = historyEntries(history);
  assert.equal(entries[0]?.role, 'assistant');
  assert.equal(entries[0]?.content, 'other text');
  assert.equal(entries[0]?.failed, false);
  assert.equal(entries[0]?.timestamp, 123);
  assert.deepEqual(entries[0]?.payload, history.messages[0]?.payload);
  const reasoningOnly: AgentHistory = { ...history, messages: [{
    ...history.messages[0]!, content: '', payload: { type: 'assistant', reasoning: 'Partial reasoning', content: null, tool_calls: [], stop_reason: 'cancelled' },
  }] };
  const reasoningEntry = historyEntries(reasoningOnly)[0]!;
  assert.equal(reasoningEntry.thinking, 'Partial reasoning');
  assert.equal(reasoningEntry.content, '');
  assert.equal('aborted' in reasoningEntry, false);
  const empty = historyEntries({ ...reasoningOnly, messages: [{ ...reasoningOnly.messages[0]!, payload: { type: 'assistant', reasoning: null, content: null, tool_calls: [], stop_reason: 'cancelled' } }] })[0]!;
  assert.equal(empty.content, '');
  assert.equal(empty.thinking, '');
});

test('abort uses independent API and ignores mismatched correlation', async () => {
  setActivePinia(createPinia());
  const store = useWorkbenchStore();
  const sent: Array<{ type: string; id: string; message: { turn_id: string } }> = [];
  const oldSocket = globalThis.WebSocket;
  class Socket {
    static OPEN = 1;
    readyState = 1;
    onopen = null;
    onclose = null;
    onmessage = null;
    onerror = null;
    close() {}
    send(raw: string) {
      sent.push(JSON.parse(raw));
    }
  }
  globalThis.WebSocket = Socket as unknown as typeof WebSocket;
  try {
    store.handleServerMessage({
      type: 'state.sync',
      state: { ...state, agents: [{ ...state.agents[0]!, status: 'working', turn: 'turn-1' }] },
    });
    store.connect();
    store.handleServerMessage({
      type: 'connection.registered',
      id: 'r',
      client: { resource_id: 'client:test', client_type: 'webui', name: 'test' },
    });
    const delta = {
      type: 'agent.message.delta' as const,
      id: 'aborted-inference',
      agent: workspace.manager,
      agent_instance: state.agents[0]!.agent_instance,
      sequence: 1,
      content: 'partial',
    };
    store.handleServerMessage(delta);
    store.handleServerMessage({ ...delta, type: 'agent.message.reasoning_delta', sequence: 2, content: 'thought' });
    const promise = store.abortTurn();
    const request = sent.find((item) => item.type === 'agent.turn.abort')!;
    assert.equal(request.message.turn_id, 'turn-1');
    assert.equal(store.abortPending, true);
    await assert.rejects(store.abortTurn(), /No abortable/);
    // Click-time suppression, before even a mismatched acknowledgement arrives.
    store.handleServerMessage({ ...delta, sequence: 3, content: ' pre-ack output' });
    store.handleServerMessage({ ...delta, type: 'agent.message.reasoning_delta', sequence: 4, content: ' pre-ack thought' });
    store.handleServerMessage({ ...delta, id: 'unseen-old', sequence: 1, content: 'unseen output' });
    store.handleServerMessage({ ...delta, id: 'unseen-reasoning', type: 'agent.message.reasoning_delta', sequence: 1, content: 'unseen thought' });
    assert.equal(store.visibleMessages.length, 1);
    assert.equal(store.visibleMessages[0]?.content, 'partial');
    assert.equal(store.visibleMessages[0]?.thinking, 'thought');
    store.handleServerMessage({
      type: 'agent.turn.abort_result',
      id: request.id,
      workspace,
      agent: workspace.manager,
      turn_id: 'wrong',
      result: { Ok: null },
    });
    assert.equal(store.abortPending, true);
    store.handleServerMessage({
      type: 'agent.turn.abort_result',
      id: request.id,
      workspace,
      agent: workspace.manager,
      turn_id: 'turn-1',
      result: { Ok: null },
    });
    await promise;
    store.handleServerMessage({ ...delta, sequence: 3, content: ' late output' });
    store.handleServerMessage({ ...delta, type: 'agent.message.reasoning_delta', sequence: 4, content: ' late thought' });
    assert.equal(store.visibleMessages[0]?.content, 'partial');
    assert.equal(store.visibleMessages[0]?.thinking, 'thought');
    assert.equal(store.selectedAgentIdle, false);
    assert.equal(store.abortPending, true);
    store.handleServerMessage({
      type: 'state.sync',
      state: { ...state, agents: [{ ...state.agents[0]!, status: 'working', turn: null }] },
    });
    assert.equal(store.abortPending, true);
    store.handleServerMessage({ type: 'state.sync', state });
    assert.equal(store.abortPending, false);
    assert.equal(store.selectedAgentIdle, true);
    store.handleServerMessage({
      type: 'state.sync',
      state: { ...state, agents: [{ ...state.agents[0]!, status: 'working', turn: 'turn-2' }] },
    });
    store.handleServerMessage({ ...delta, sequence: 5, content: ' stale next turn' });
    store.handleServerMessage({ ...delta, id: 'unseen-old', sequence: 2, content: ' stale unseen' });
    store.handleServerMessage({ ...delta, id: 'unseen-reasoning', type: 'agent.message.reasoning_delta', sequence: 2, content: ' stale unseen thought' });
    assert.equal(store.visibleMessages[0]?.content, 'partial');
    store.handleServerMessage({ ...delta, id: 'new-inference', sequence: 1, content: 'new output' });
    assert.equal(store.visibleMessages[1]?.content, 'new output');
    // Suppression does not consume the real terminal event or authoritative history.
    store.handleServerMessage({
      type: 'agent.message.finished', id: delta.id, agent: delta.agent,
      agent_instance: delta.agent_instance, error: null,
    });
    store.handleServerMessage({
      type: 'history.result', id: 'history',
      history: { workspace, agent: workspace.manager, messages: [{
        key: 'persisted-partial', sequence: 1, turn: 'turn-1', kind: 'message',
        content: 'partial', created_at_ms: 0,
        payload: { type: 'assistant', content: 'partial', reasoning: 'thought', tool_calls: [], usage: null, stop_reason: 'cancelled' },
      }] },
    });
    assert.equal(store.visibleMessages[0]?.content, 'partial');
    assert.equal('aborted' in store.visibleMessages[0]!, false);
    assert.equal(store.visibleMessages[1]?.content, 'new output');
    store.handleServerMessage({ ...delta, sequence: 6, content: ' resurrected' });
    assert.equal(store.visibleMessages.length, 2);
  } finally {
    store.disconnect();
    globalThis.WebSocket = oldSocket;
  }
});


test('image contract strips UI names and reconstructs backend image-only history', () => {
  const image = { media_type: 'image/png', data: 'iVBORw0KGgo=', name: 'private.png', size: 8 };
  const images = wireImages([image]);
  assert.deepEqual(images, [{ media_type: 'image/png', data: 'iVBORw0KGgo=' }]);
  assert.deepEqual(contextInjection('', 'entry.pending[-1]', [image]).arguments,
    [[{ type: 'user', content: '', images }]]);
  const entries = historyEntries({ workspace, agent: workspace.manager, messages: [{
    key: 'image', sequence: 1, turn: null, kind: 'message', content: '', created_at_ms: 1,
    payload: { type: 'user', images },
  }] });
  assert.equal(entries[0]?.role, 'user');
  assert.equal(entries[0]?.content, '');
  assert.deepEqual(entries[0]?.images, images);
  assert.ok(!metadataText({ messages: [{ images }] }).includes(image.data));
  assert.equal(isAgentMessage({ type: 'user', content: '', images: [{ media_type: 'image/svg+xml', data: 'abcd' }] }), false);
});

test('image budgets and magic reject unsupported and oversized imports', async () => {
  const mib = 1024 * 1024;
  validateImageBudget(Array(8).fill(2 * mib));
  validateImageBudget(Array(4).fill(5 * mib));
  assert.throws(() => validateImageBudget(Array(9).fill(1)), /8 images/);
  assert.throws(() => validateImageBudget([5 * mib + 1]), /5 MiB/);
  assert.throws(() => validateImageBudget(Array(5).fill(5 * mib)), /20 MiB/);
  assert.equal(imageMediaType(new Uint8Array([137,80,78,71,13,10,26,10])), 'image/png');
  assert.equal(imageMediaType(new Uint8Array([255,216,255])), 'image/jpeg');
  assert.equal(imageMediaType(new TextEncoder().encode('GIF89a')), 'image/gif');
  assert.equal(imageMediaType(new TextEncoder().encode('RIFFxxxxWEBP')), 'image/webp');
  assert.equal(imageMediaType(new TextEncoder().encode('<svg />')), null);
  await assert.rejects(importImage(new File(['<svg />'], 'fake.png', { type: 'image/png' })), /Invalid image/);
  await assert.rejects(importImage(new File(['text'], 'a.svg', { type: 'image/svg+xml' })), /Use PNG/);
});

test('image-only send uses MCL and route switching prevents late wake even after switching back', async () => {
  setActivePinia(createPinia());
  const store = useWorkbenchStore();
  const oldSocket = globalThis.WebSocket;
  const sent: Array<{ type: string; id: string; message: { command: string; agent: string; arguments: unknown[][] } }> = [];
  class Socket {
    static OPEN = 1;
    readyState = 1;
    close() {}
    send(raw: string) {
      const request = JSON.parse(raw) as typeof sent[number];
      sent.push(request);
      if (request.message.command?.startsWith('GET ')) queueMicrotask(() => reply(request.id));
    }
  }
  function reply(id: string) {
    store.handleServerMessage({ type: 'mcl.command_result', id, source: 'client:test', target: workspace.manager, result: { Ok: 'Unit' }, history_error: null });
  }
  globalThis.WebSocket = Socket as unknown as typeof WebSocket;
  try {
    store.handleServerMessage({ type: 'state.sync', state });
    store.connect();
    store.handleServerMessage({ type: 'connection.registered', id: 'r', client: { resource_id: 'client:test', name: 'test', client_type: 'webui' } });
    const image = { media_type: 'image/png', data: 'iVBORw0KGgo=' };
    const sending = store.sendMessage('', [image]);
    const injected = sent.find((request) => request.message.command?.startsWith('INJECT'))!;
    assert.equal(injected.message.agent, workspace.manager);
    assert.deepEqual(injected.message.arguments, [[{ type: 'user', content: '', images: [image] }]]);
    assert.equal(store.visibleMessages.length, 0);
    store.selectAgent('agent:other');
    store.selectAgent(null);
    reply(injected.id);
    await assert.rejects(sending, /changed/);
    assert.ok(!sent.some((request) => request.message.command === 'EMIT EFFECT wake'));
  } finally { store.disconnect(); globalThis.WebSocket = oldSocket; }
});


test('multi-MiB serialized images wait for parsed acknowledgements and recover after rejection', async () => {
  const oldSocket = globalThis.WebSocket;
  // Transport-size fixture, not a browser decode fixture: exceeds the former 1 MiB daemon cutoff.
  const image = { media_type: 'image/png', data: 'A'.repeat(2 * 1024 * 1024) };
  try {
    for (const failure of [false, true]) {
      setActivePinia(createPinia());
      const store = useWorkbenchStore();
      const sent: Array<{ type: string; id: string; message: { command?: string; arguments?: unknown[][] } }> = [];
      const sockets: Socket[] = [];
      class Socket {
        static OPEN = 1;
        readyState = 1;
        onmessage: ((event: { data: string }) => void) | null = null;
        constructor() { sockets.push(this); }
        close() {}
        send(raw: string) {
          const request = JSON.parse(raw) as typeof sent[number];
          sent.push(request);
          if (request.message.command?.startsWith('GET ')) queueMicrotask(() => acknowledge(request.id));
        }
      }
      function receive(event: unknown) { sockets.at(-1)?.onmessage?.({ data: JSON.stringify(event) }); }
      function acknowledge(id: string) {
        receive({ type: 'mcl.command_result', id, source: 'client:test', target: workspace.manager,
          result: { Ok: 'Unit' }, history_error: null });
      }
      globalThis.WebSocket = Socket as unknown as typeof WebSocket;
      try {
        store.handleServerMessage({ type: 'state.sync', state });
        store.connect();
        receive({ type: 'connection.registered', id: 'r', client: { resource_id: 'client:test', name: 'test', client_type: 'webui' } });
        const stages: string[] = [];
        const sending = store.sendMessage('', [image], (stage) => stages.push(stage));
        const injected = sent.find((request) => request.message.command?.startsWith('INJECT'))!;
        assert.deepEqual(injected.message.arguments, [[{ type: 'user', content: '', images: [image] }]]);
        assert.ok(JSON.stringify(injected).length > 1024 * 1024);
        assert.deepEqual(stages, ['injecting']);
        assert.ok(!sent.some((request) => request.message.command === 'EMIT EFFECT wake'));
        if (failure) {
          receive({ type: 'request.failed', id: injected.id, error: { kind: 'MessageTooLarge', message: 'Message exceeds transport budget' } });
          await assert.rejects(sending, /transport budget/);
          assert.ok(!sent.some((request) => request.message.command === 'EMIT EFFECT wake'));
          assert.match(store.lastFailures.get(`${workspace.id}:${workspace.manager}`)!.message, /transport budget/);
          const retry = store.sendMessage('retry');
          acknowledge(sent.filter((request) => request.message.command?.startsWith('INJECT')).at(-1)!.id);
          await Promise.resolve();
          acknowledge(sent.find((request) => request.message.command === 'EMIT EFFECT wake')!.id);
          await retry;
        } else {
          acknowledge(injected.id);
          await Promise.resolve();
          assert.deepEqual(stages, ['injecting', 'waking']);
          acknowledge(sent.find((request) => request.message.command === 'EMIT EFFECT wake')!.id);
          await sending;
          receive({ type: 'history.result', id: 'h', history: { workspace, agent: workspace.manager, messages: [{
            key: 'image', sequence: 1, turn: null, kind: 'message', content: '', created_at_ms: 1,
            payload: { type: 'user', images: [image] },
          }] } });
          assert.equal(store.visibleMessages[0]?.images?.[0]?.data.length, image.data.length);
        }
        const diagnostics = JSON.stringify(store.logs.filter((entry) => entry.target === 'ui.send'));
        assert.ok(!diagnostics.includes(image.data));
        assert.ok(diagnostics.includes('acknowledgement'));
      } finally { store.disconnect(); }
    }
  } finally { globalThis.WebSocket = oldSocket; }
});


test('correlated abort rejection releases only the still-current turn suppression', async () => {
  const oldSocket = globalThis.WebSocket;
  const sent: Array<{ type: string; id: string }> = [];
  class Socket {
    static OPEN = 1;
    readyState = 1;
    onopen = null;
    onclose = null;
    onmessage = null;
    onerror = null;
    close() {}
    send(raw: string) { sent.push(JSON.parse(raw)); }
  }
  globalThis.WebSocket = Socket as unknown as typeof WebSocket;
  for (const outcome of ['same-turn', 'new-turn', 'request-failed'] as const) {
    setActivePinia(createPinia());
    const store = useWorkbenchStore();
    try {
      const sync = (turn: string) => store.handleServerMessage({ type: 'state.sync',
        state: { ...state, agents: [{ ...state.agents[0]!, status: 'working', turn }] } });
      sync('turn-1');
      store.connect();
      store.handleServerMessage({ type: 'connection.registered', id: 'r',
        client: { resource_id: 'client:test', client_type: 'webui', name: 'test' } });
      const delta = { type: 'agent.message.delta' as const, id: 'known', agent: workspace.manager,
        agent_instance: state.agents[0]!.agent_instance, sequence: 1, content: 'partial' };
      store.handleServerMessage(delta);
      const promise = store.abortTurn();
      const rejection = assert.rejects(promise, /denied/);
      const request = sent.filter((event) => event.type === 'agent.turn.abort').at(-1)!;
      store.handleServerMessage({ ...delta, id: 'unseen', content: 'hidden' });
      assert.equal(store.visibleMessages.length, 1);
      if (outcome === 'new-turn') sync('turn-2');
      if (outcome === 'request-failed') {
        store.handleServerMessage({ type: 'request.failed', id: request.id, error: { kind: 'abort_rejected', message: 'denied' } });
      } else {
        store.handleServerMessage({ type: 'agent.turn.abort_result', id: request.id, workspace,
          agent: workspace.manager, turn_id: 'turn-1', result: { Err: { kind: 'abort_rejected', message: 'denied' } } });
      }
      await rejection;
      assert.equal(store.abortPending, false);
      assert.equal(store.selectedAgentIdle, false);
      store.handleServerMessage({ ...delta, sequence: 2, content: ' resumed' });
      store.handleServerMessage({ ...delta, id: 'unseen', sequence: 2, content: 'resumed unseen' });
      if (outcome === 'new-turn') {
        assert.equal(store.visibleMessages.length, 1);
        assert.equal(store.visibleMessages[0]?.content, 'partial');
        store.handleServerMessage({ ...delta, id: 'fresh', content: 'new output' });
        assert.equal(store.visibleMessages[1]?.content, 'new output');
      } else {
        assert.equal(store.visibleMessages[0]?.content, 'partial resumed');
        assert.equal(store.visibleMessages[1]?.content, 'resumed unseen');
      }
    } finally { store.disconnect(); }
  }
  globalThis.WebSocket = oldSocket;
});


test('tool image history retains tool identity and image metadata', () => {
  const image = {media_type:'image/png',data:'iVBORw0KGgo='};
  const entry = historyEntries({workspace,agent:workspace.manager,messages:[{
    key:'read-image',sequence:1,turn:'turn-image',kind:'message',content:'Read image screenshot.png',
    payload:{type:'tool_call',tool_call_id:'read-1',tool_name:'read_image',resource_id:'tool:local/read-image:latest',failed:false,images:[image]},created_at_ms:123,
  }]})[0]!;
  assert.equal(entry.role,'tool');
  assert.equal(entry.content,'Read image screenshot.png');
  assert.deepEqual(entry.images,[image]);
  assert.equal((entry.payload as {tool_call_id:string}).tool_call_id,'read-1');
  assert.equal(entry.failed,false);
});
