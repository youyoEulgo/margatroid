/// <reference types="node" />
import { strict as assert } from 'node:assert';
import { test } from 'node:test';
import { createPinia, setActivePinia } from 'pinia';
import { createRenderer, h, markRaw, nextTick } from 'vue';
import SidebarTree from '../components/SidebarTree.vue';
import {
  contextInjection,
  buildSidebarModel,
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
  agents: ['agent:local/coder:latest'],
};
const state: BackendState = {
  catalog: [{ id: workspace.id, name: workspace.name, path: '/catalog/test.yaml', revision: 'rev-1', lifecycle: 'running', definition: { ...workspace, agents: [{ id: workspace.agents[0]!, name: 'coder', driver: 'driver:local/coder:latest' }] } }],
  workspaces: [workspace],
  agents: [
    {
      workspace,
      agent: workspace.agents[0]!,
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
        agent: workspace.agents[0]!,
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
        agent: workspace.agents[0]!,
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
      target: workspace.agents[0]!,
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
        target: workspace.agents[0]!,
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
    agent: workspace.agents[0]!,
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
    agent: workspace.agents[0]!,
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
    agent: workspace.agents[0]!,
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
    history: { workspace, agent: workspace.agents[0]!, messages: [] },
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
    agent: workspace.agents[0]!,
    agent_instance: 'Entity(1, 2)',
    error: { kind: 'InferenceFailed', message: 'real failure' },
  });
  assert.deepEqual(store.lastFailures.get(`${workspace.id}:${workspace.agents[0]!}`), {
    message: 'real failure',
  });
  store.handleServerMessage({
    type: 'agent.message.finished',
    id: 'stale',
    agent: workspace.agents[0]!,
    agent_instance: 'old',
    error: { kind: 'InferenceFailed', message: 'stale failure' },
  });
  assert.equal(
    store.lastFailures.get(`${workspace.id}:${workspace.agents[0]!}`)?.message,
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
          target: workspace.agents[0]!,
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
            agent: workspace.agents[0]!,
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
      agent: workspace.agents[0]!,
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
    store.lastFailures.get(`${workspace.id}:${workspace.agents[0]!}`)?.message,
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
    agent: workspace.agents[0]!,
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
          agent: workspace.agents[0]!,
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
    agent: workspace.agents[0]!,
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
      agent: workspace.agents[0]!,
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
      agent: workspace.agents[0]!,
      turn_id: 'wrong',
      result: { Ok: null },
    });
    assert.equal(store.abortPending, true);
    store.handleServerMessage({
      type: 'agent.turn.abort_result',
      id: request.id,
      workspace,
      agent: workspace.agents[0]!,
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
      history: { workspace, agent: workspace.agents[0]!, messages: [{
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
  const entries = historyEntries({ workspace, agent: workspace.agents[0]!, messages: [{
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
    store.handleServerMessage({ type: 'mcl.command_result', id, source: 'client:test', target: workspace.agents[0]!, result: { Ok: 'Unit' }, history_error: null });
  }
  globalThis.WebSocket = Socket as unknown as typeof WebSocket;
  try {
    store.handleServerMessage({ type: 'state.sync', state });
    store.connect();
    store.handleServerMessage({ type: 'connection.registered', id: 'r', client: { resource_id: 'client:test', name: 'test', client_type: 'webui' } });
    const image = { media_type: 'image/png', data: 'iVBORw0KGgo=' };
    const sending = store.sendMessage('', [image]);
    const injected = sent.find((request) => request.message.command?.startsWith('INJECT'))!;
    assert.equal(injected.message.agent, workspace.agents[0]!);
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
        receive({ type: 'mcl.command_result', id, source: 'client:test', target: workspace.agents[0]!,
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
          assert.match(store.lastFailures.get(`${workspace.id}:${workspace.agents[0]!}`)!.message, /transport budget/);
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
          receive({ type: 'history.result', id: 'h', history: { workspace, agent: workspace.agents[0]!, messages: [{
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
      const delta = { type: 'agent.message.delta' as const, id: 'known', agent: workspace.agents[0]!,
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
          agent: workspace.agents[0]!, turn_id: 'turn-1', result: { Err: { kind: 'abort_rejected', message: 'denied' } } });
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
  const entry = historyEntries({workspace,agent:workspace.agents[0]!,messages:[{
    key:'read-image',sequence:1,turn:'turn-image',kind:'message',content:'Read image screenshot.png',
    payload:{type:'tool_call',tool_call_id:'read-1',tool_name:'read_image',resource_id:'tool:local/read-image:latest',failed:false,images:[image]},created_at_ms:123,
  }]})[0]!;
  assert.equal(entry.role,'tool');
  assert.equal(entry.content,'Read image screenshot.png');
  assert.deepEqual(entry.images,[image]);
  assert.equal((entry.payload as {tool_call_id:string}).tool_call_id,'read-1');
  assert.equal(entry.failed,false);
});


test('catalog keeps stopped invalid and live entries with zero sole and multiple explicit routing', async () => {
  setActivePinia(createPinia());
  const store = useWorkbenchStore();
  const sole = state.catalog[0]!;
  const empty = { ...sole, id: 'workspace:local/empty:latest', name: 'empty', lifecycle: 'stopped', definition: { ...sole.definition!, id: 'workspace:local/empty:latest', agents: [] } };
  const invalid = { id: 'workspace:local/bad:latest', name: 'bad', path: '/catalog/bad/workspace.yaml', revision: 'broken', lifecycle: 'stopped', error: 'Invalid YAML' };
  const multi = { ...sole, id: 'workspace:local/multi:latest', name: 'multi', lifecycle: 'stopped', definition: { ...sole.definition!, id: 'workspace:local/multi:latest', agents: [...sole.definition!.agents, { id: 'agent:local/reviewer:latest', name: 'reviewer', driver: 'driver:local/review:latest' }] } };
  store.handleServerMessage({ type: 'state.sync', state: { ...state, catalog: [empty, invalid, sole, multi] } });
  assert.equal(store.workspaces.length, 4);
  assert.equal(store.selectedAgent, null);
  assert.equal(store.selectedAgentReady, false);
  await assert.rejects(store.executeMclCommand('GET session.visible'));
  store.selectWorkspace(invalid.id);
  assert.equal(store.selectedCatalogEntry?.error, 'Invalid YAML');
  await assert.rejects(store.startWorkspace(), /Fix/);
  store.selectWorkspace(sole.id);
  assert.equal(store.selectedAgent, sole.definition!.agents[0]!.id);
  store.selectWorkspace(multi.id);
  assert.equal(store.selectedAgent, null);
  store.selectAgent('agent:local/reviewer:latest');
  store.handleServerMessage({ type: 'state.sync', state: { ...state, catalog: [empty, invalid, sole, multi] } });
  assert.equal(store.selectedWorkspace?.id, multi.id);
  assert.equal(store.selectedAgent, 'agent:local/reviewer:latest');
  store.handleServerMessage({ type: 'workspace.catalog', id: 'refresh', entries: [empty, invalid, sole, { ...multi, definition: { ...multi.definition, agents: [] } }] });
  assert.equal(store.selectedAgent, null);
});

test('catalog mutations use revisions and wait for authoritative results without optimistic members', async () => {
  setActivePinia(createPinia());
  const store = useWorkbenchStore();
  const oldSocket = globalThis.WebSocket;
  const sent: Array<{ type: string; id: string; message: Record<string, unknown> }> = [];
  class Socket {
    static OPEN = 1; readyState = 1; onopen = null; onclose = null; onmessage = null; onerror = null;
    close() {}
    send(raw: string) { sent.push(JSON.parse(raw)); }
  }
  globalThis.WebSocket = Socket as unknown as typeof WebSocket;
  try {
    const entry = { ...state.catalog[0]!, lifecycle: 'stopped', definition: { ...state.catalog[0]!.definition!, agents: [] } };
    store.handleServerMessage({ type: 'state.sync', state: { catalog: [entry], drivers: ['driver:local/coder:latest'], workspaces: [], agents: [], histories: [] } });
    store.connect();
    store.handleServerMessage({ type: 'connection.registered', id: 'register', client: { resource_id: 'client:test', name: 'test', client_type: 'webui' } });
    assert.ok(sent.some((request) => request.type === 'workspace.catalog.query'));
    const adding = store.addMember('coder', 'driver:local/coder:latest');
    const add = sent.at(-1)!;
    assert.equal(add.type, 'workspace.member.add');
    assert.deepEqual(add.message, { workspace: entry.id, expected_revision: entry.revision, agent: { name: 'coder', driver: 'driver:local/coder:latest' } });
    assert.equal(store.selectedWorkspace?.agents.length, 0);
    const saved = { ...entry, revision: 'rev-2', definition: { ...entry.definition, agents: state.catalog[0]!.definition!.agents } };
    store.handleServerMessage({ type: 'workspace.member.result', id: add.id, entry: saved });
    await adding;
    assert.equal(store.selectedAgent, saved.definition.agents[0]!.id);
    assert.match(store.workspaceNotice, /persist|saved/i);
    const removing = store.removeMember(store.selectedAgent!);
    const remove = sent.at(-1)!;
    assert.equal(remove.message.expected_revision, 'rev-2');
    assert.equal(store.selectedWorkspace?.agents.length, 1);
    store.handleServerMessage({ type: 'workspace.member.result', id: remove.id, entry: null, error: { kind: 'RevisionConflict', message: 'Definition changed; refresh and retry' } });
    await assert.rejects(removing, /Definition changed/);
    assert.equal(store.selectedWorkspace?.agents.length, 1);
    assert.equal(sent.at(-1)?.type, 'workspace.catalog.query');
    const starting = store.startWorkspace();
    const start = sent.at(-1)!;
    assert.deepEqual(start.message, { workspace: entry.id, expected_revision: 'rev-2' });
    assert.equal(store.selectedCatalogEntry?.lifecycle, 'stopped');
    store.handleServerMessage({ type: 'workspace.start_failed', id: start.id, error: { kind: 'StartFailed', message: 'Could not start' } });
    await assert.rejects(starting, /Could not start/);
  } finally { store.disconnect(); globalThis.WebSocket = oldSocket; }
});


test('sidebar dots distinguish transitions, healthy empty runtimes, current failures and recovery', () => {
  const entry = state.catalog[0]!;
  const model = (lifecycle: string, status: typeof state.agents[number]['status'], error?: {kind: string; message: string}) => buildSidebarModel(
    [{ ...entry, lifecycle }], state.workspaces, [{ ...state.agents[0]!, status, error }], new Map(), new Set([entry.id]), entry.id, null).workspaces[0]!;
  assert.equal(model('running', 'idle').status, 'running');
  assert.equal(model('running', 'working').agents[0]!.status, 'running');
  for (const status of ['creating', 'stopping'] as const) {
    assert.equal(model('running', status).agents[0]!.status, 'stopped');
    assert.equal(model('running', status).status, 'stopped');
  }
  assert.equal(model('starting', 'creating').status, 'stopped');
  assert.equal(model('stopping', 'stopping').status, 'stopped');
  assert.equal(model('running', 'idle', {kind: 'DriverFailed', message: 'failed'}).status, 'error');
  const empty = { ...entry, definition: { ...entry.definition!, agents: [] } };
  assert.equal(buildSidebarModel([empty], state.workspaces, [], new Map(), new Set(), '', null).workspaces[0]!.status, 'running');
  setActivePinia(createPinia());
  const store = useWorkbenchStore();
  store.handleServerMessage({ type: 'state.sync', state: { ...state, agents: [{ ...state.agents[0]!, error: { kind: 'Error', message: 'failed' } }] } });
  assert.equal(store.sidebarTree.workspaces[0]!.status, 'error');
  store.handleServerMessage({ type: 'state.sync', state });
  assert.equal(store.sidebarTree.workspaces[0]!.status, 'running');
  store.toggleWorkspace(entry.id);
  assert.equal(store.isWorkspaceExpanded(entry.id), false);
  store.handleServerMessage({ type: 'state.sync', state });
  assert.equal(store.isWorkspaceExpanded(entry.id), false);
});

test('driver and folder replies parse and reject malformed fields', () => {
  for (const message of [
    { type: 'driver.catalog', id: 'drivers', drivers: ['driver:local/unmounted:latest'], errors: [{path: '/invalid', message: 'Bad package'}] },
    { type: 'workspace.paths', id: 'paths', path: '/home/me', parent: '/home', directories: [{name: 'project', path: '/home/me/project'}], next_cursor: 'page2' },
    { type: 'workspace.paths', id: 'root', path: '/', parent: null, directories: [] },
  ]) assert.deepEqual(parseServerMessage(JSON.stringify(message)), message);
  assert.equal(parseServerMessage(JSON.stringify({type: 'driver.catalog', id: 'x', drivers: [1], errors: []})), null);
  assert.equal(parseServerMessage(JSON.stringify({type: 'workspace.paths', id: 'x', path: '/', parent: null, directories: [{name: 'x'}]})), null);
  assert.equal(parseServerMessage(JSON.stringify({type: 'workspace.paths', id: 'x', path: '/', parent: null, directories: [], next_cursor: 1})), null);
  assert.equal(parseServerMessage(JSON.stringify({type: 'workspace.member.result', id: 'delete', entry: null}))?.type, 'workspace.member.result');
});

test('catalog operations capture nested targets, preserve other selections and rebind renamed IDs', async () => {
  setActivePinia(createPinia());
  const store = useWorkbenchStore();
  const previousSocket = globalThis.WebSocket;
  const sent: Array<{ type: string; id: string; message: Record<string, unknown> }> = [];
  class Socket {
    static OPEN = 1; readyState = 1; onopen = null; onclose = null; onmessage = null; onerror = null;
    close() {} send(raw: string) { sent.push(JSON.parse(raw)); }
  }
  globalThis.WebSocket = Socket as unknown as typeof WebSocket;
  const original = { ...state.catalog[0]!, lifecycle: 'stopped' };
  const other = { ...original, id: 'workspace:local/other:latest', name: 'other', definition: { ...original.definition!, id: 'workspace:local/other:latest', agents: [] } };
  try {
    store.handleServerMessage({type: 'state.sync', state: {catalog: [original, other], drivers: ['driver:local/coder:latest'], workspaces: [], agents: [], histories: []}});
    store.connect();
    store.handleServerMessage({type: 'connection.registered', id: 'registered', client: {resource_id: 'client:test', name: 'test', client_type: 'webui'}});
    const target = {key: original.id, revision: original.revision};
    store.selectWorkspace(other.id);
    const adding = store.addMember('new', 'driver:local/coder:latest', target);
    const add = sent.at(-1)!;
    assert.equal(add.message.workspace, original.id);
    const added = {...original, revision: 'rev2', definition: {...original.definition!, agents: [...original.definition!.agents, {id: 'agent:local/new:latest', name: 'new', driver: 'driver:local/coder:latest'}]}};
    store.handleServerMessage({type: 'workspace.member.result', id: add.id, entry: added});
    await adding;
    assert.equal(store.selectedWorkspaceKey, other.id);
    assert.equal(store.selectedAgent, null);
    assert.equal(sent.at(-1)!.type, 'workspace.catalog.query');
    store.selectAgent('agent:local/new:latest', original.id);
    const deletingSelected = store.removeMember('agent:local/new:latest', target);
    const selectedDelete = sent.at(-1)!;
    store.handleServerMessage({type: 'workspace.member.result', id: selectedDelete.id, entry: original}); await deletingSelected;
    store.handleServerMessage({type: 'workspace.catalog', id: 'after-member-delete', entries: [original, other]});
    assert.equal(store.selectedAgent, null, 'deleting a selection must not select its remaining sibling');
    store.selectWorkspace(other.id);
    const removing = store.removeMember('agent:local/new:latest', {key: original.id, revision: 'captured-revision'});
    const remove = sent.at(-1)!;
    assert.deepEqual(remove.message, {workspace: original.id, expected_revision: 'captured-revision', agent: 'agent:local/new:latest'});
    store.handleServerMessage({type: 'workspace.member.result', id: remove.id, entry: original}); await removing;
    store.selectWorkspace(original.id);
    const renaming = store.renameMember(original.definition!.agents[0]!.id, 'renamed', target);
    const rename = sent.at(-1)!;
    const renamed = {...original, revision: 'rev3', definition: {...original.definition!, agents: [{...original.definition!.agents[0]!, id: 'agent:local/renamed:latest', name: 'renamed'}]}};
    assert.equal(store.selectedAgent, original.definition!.agents[0]!.id);
    store.handleServerMessage({type: 'state.sync', state: {catalog: [renamed, other], workspaces: [], agents: [], histories: []}});
    store.handleServerMessage({type: 'workspace.member.result', id: rename.id, entry: renamed}); await renaming;
    assert.equal(store.selectedAgent, 'agent:local/renamed:latest');
    const workspaceRenaming = store.renameWorkspace(original.id, 'renamed-workspace', 'rev3');
    const renameWorkspace = sent.at(-1)!;
    const newWorkspace = {...renamed, id: 'workspace:local/renamed-workspace:latest', name: 'renamed-workspace', definition: {...renamed.definition, id: 'workspace:local/renamed-workspace:latest'}};
    store.handleServerMessage({type: 'state.sync', state: {catalog: [newWorkspace, other], workspaces: [], agents: [], histories: []}});
    store.handleServerMessage({type: 'workspace.member.result', id: renameWorkspace.id, entry: newWorkspace}); await workspaceRenaming;
    assert.equal(store.selectedWorkspaceKey, newWorkspace.id);
    assert.equal(store.catalog.some((entry) => entry.id === original.id), false);
    assert.equal(store.isWorkspaceExpanded(original.id), false);
    const deleting = store.deleteWorkspace(newWorkspace.id);
    const deletion = sent.at(-1)!;
    store.handleServerMessage({type: 'workspace.member.result', id: deletion.id, entry: null}); await deleting;
    assert.equal(store.selectedWorkspaceKey, '');
    assert.equal(store.selectedAgent, null);
    store.handleServerMessage({type: 'workspace.catalog', id: 'authoritative', entries: [other]});
    assert.equal(store.selectedWorkspaceKey, '');
    await assert.rejects(store.addMember('bad', 'driver:local/unvalidated:latest', {key: other.id, revision: other.revision}), /validated/);
    const drivers = store.queryDrivers(); const driverRequest = sent.at(-1)!;
    assert.equal(driverRequest.type, 'driver.catalog.query');
    store.handleServerMessage({type: 'driver.catalog', id: driverRequest.id, drivers: ['driver:local/unmounted:latest', 'garbage'], errors: [{path: '/bad', message: 'Invalid manifest'}]}); await drivers;
    assert.deepEqual(store.drivers, ['driver:local/unmounted:latest']);
    assert.equal(store.driverCatalogErrors[0]!.message, 'Invalid manifest');
    const refreshing = store.queryDrivers(true); const refresh = sent.at(-1)!;
    assert.equal(refresh.type, 'driver.catalog.refresh');
    store.handleServerMessage({type: 'driver.catalog', id: refresh.id, drivers: [], errors: []}); await refreshing;
    assert.deepEqual(store.drivers, []);
    const folders = store.queryPaths({path: '/home', cursor: 'next'}); const paths = sent.at(-1)!;
    assert.deepEqual(paths.message, {path: '/home', cursor: 'next'});
    store.handleServerMessage({type: 'workspace.paths', id: paths.id, path: '/home', parent: '/', directories: [{name: 'project', path: '/home/project'}], next_cursor: 'last'});
    assert.equal((await folders).directories[0]!.path, '/home/project');
  } finally { store.disconnect(); globalThis.WebSocket = previousSocket; }
});


test('nested row menus contain only Rename/Delete, capture targets and close with keyboard or outside clicks', async () => {
  class Host {
    parent: Host | null = null; children: Host[] = []; props: Record<string, unknown> = {}; text = '';
    constructor(public type: string) { markRaw(this); }
    focus() { documentMock.activeElement = this; }
    contains(target: Host): boolean { return this === target || this.children.some((child) => child.contains(target)); }
    querySelectorAll(): Host[] { return descendants(this).filter((node) => node.props.role === 'menuitem'); }
    querySelector(): Host | null { return this.querySelectorAll()[0] ?? null; }
    closest(): Host | null { return this.props.role === 'menu' ? this : this.parent?.closest() ?? null; }
  }
  const descendants = (node: Host): Host[] => node.children.flatMap((child) => [child, ...descendants(child)]);
  const listeners = new Map<string, (event: unknown) => void>();
  const documentMock = {
    activeElement: null as Host | null,
    addEventListener: (name: string, listener: (event: unknown) => void) => listeners.set(name, listener),
    removeEventListener: (name: string) => listeners.delete(name),
  };
  const globals = ['document', 'HTMLElement', 'Element', 'Node', 'CSS'] as const;
  const descriptors = globals.map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)] as const);
  Object.defineProperties(globalThis, Object.fromEntries([
    ['document', { value: documentMock, configurable: true }],
    ...['HTMLElement', 'Element', 'Node'].map((key) => [key, { value: Host, configurable: true }]),
    ['CSS', { value: { escape: (text: string) => text }, configurable: true }],
  ]));
  const renderer = createRenderer<Host, Host>({
    createElement: (type) => new Host(type), createText: (text) => Object.assign(new Host('text'), {text}),
    createComment: (text) => Object.assign(new Host('comment'), {text}),
    setText: (node, text) => { node.text = text; }, setElementText: (node, text) => { node.text = text; node.children = []; },
    parentNode: (node) => node.parent, nextSibling: (node) => node.parent?.children[node.parent.children.indexOf(node) + 1] ?? null,
    patchProp: (node, key, _previous, value) => { node.props[key] = value; },
    insert: (node, parent, anchor) => {
      if (node.parent) node.parent.children.splice(node.parent.children.indexOf(node), 1);
      node.parent = parent;
      const index = anchor ? parent.children.indexOf(anchor) : -1;
      if (index < 0) parent.children.push(node); else parent.children.splice(index, 0, node);
    },
    remove: (node) => { if (node.parent) node.parent.children.splice(node.parent.children.indexOf(node), 1); node.parent = null; },
  });
  const root = new Host('root');
  const events: unknown[][] = [];
  const entry = state.catalog[0]!;
  const other = {...entry, id: 'workspace:local/other:latest', name: 'other'};
  const model = buildSidebarModel([entry, other], state.workspaces, state.agents, new Map(), new Set([entry.id, other.id]), other.id, null);
  const app = renderer.createApp({render: () => h(SidebarTree, {model, pending: false, offline: false,
    onRenameAgent: (...target: unknown[]) => events.push(['rename-agent', ...target]),
    onDeleteWorkspace: (...target: unknown[]) => events.push(['delete-workspace', ...target]),
    onSelectAgent: (...target: unknown[]) => events.push(['select-agent', ...target]),
    onToggleWorkspace: (...target: unknown[]) => events.push(['toggle-workspace', ...target]),
  })});
  try {
    app.mount(root); await nextTick();
    const nodes = () => descendants(root);
    const click = async (node: Host) => { (node.props.onClick as (event: unknown) => void)({currentTarget: node}); await nextTick(); };
    const agentMenu = nodes().find((node) => node.props['aria-label'] === 'Agent actions for coder')!;
    await click(agentMenu);
    let items = nodes().filter((node) => node.props.role === 'menuitem');
    const text = (node: Host): string => node.text + node.children.map(text).join('');
    assert.deepEqual(items.map((item) => text(item).trim()), ['Rename', 'Delete']);
    assert.equal(documentMock.activeElement, items[0]);
    await click(items[0]!);
    assert.deepEqual(events, [['rename-agent', entry.id, entry.definition!.agents[0]!.id]]);
    assert.equal(documentMock.activeElement, agentMenu);
    await click(agentMenu);
    listeners.get('keydown')!({key: 'Escape', stopPropagation() {}}); await nextTick();
    assert.equal(nodes().some((node) => node.props.role === 'menu'), false);
    await click(agentMenu);
    listeners.get('pointerdown')!({target: new Host('outside')}); await nextTick();
    assert.equal(nodes().some((node) => node.props.role === 'menu'), false);
    const workspaceMenu = nodes().find((node) => node.props['aria-label'] === 'Workspace actions for other')!;
    await click(workspaceMenu); items = nodes().filter((node) => node.props.role === 'menuitem');
    await click(items[1]!);
    assert.deepEqual(events.at(-1), ['delete-workspace', other.id]);
    const agentRow = nodes().find((node) => node.type === 'button' && String(node.props.class).includes('tree-main--agent'))!;
    await click(agentRow);
    assert.deepEqual(events.at(-1), ['select-agent', entry.definition!.agents[0]!.id, entry.id]);
  } finally {
    app.unmount();
    for (const [key, descriptor] of descriptors) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor); else Reflect.deleteProperty(globalThis, key);
    }
  }
});
