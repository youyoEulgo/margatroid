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
