import assert from 'node:assert/strict';
import test from 'node:test';

import { due, register, restore, strip } from '../claude/hooks/jev-context-compaction.js';

test('compaction posts one copy of each result and restores what it kept', async () => {
  const handlers = new Map();
  register((name, handler) => handlers.set(name, handler), {});
  const result = { stdout: 'x'.repeat(1000) };
  const messages = [
    { role: 'user', text: 'fix it', toolUses: [], handle: 'h0' },
    ...['a', 'b'].flatMap((id) => [
      {
        role: 'assistant', text: '', handle: `c-${id}`,
        toolUses: [{ tool_use_id: id, tool: 'Bash', input: {}, result, text: 'x'.repeat(1000), agentId: id }],
      },
      {
        role: 'user', text: '', toolUses: [], handle: `r-${id}`,
        toolResults: [{ tool_use_id: id, text: 'x'.repeat(1000), isError: false, result }],
      },
    ]),
  ];
  let posted;
  const $ = {
    http: {
      fetch: async (_url, { body }) => {
        posted = JSON.parse(body).messages;
        // The server keeps call `b` and drops call `a`; it omits isError: false.
        const kept = posted.filter((message) => !message.handle.endsWith('-a')).map((message) => ({
          ...message,
          ...(message.toolResults && {
            toolResults: message.toolResults.map(({ isError, ...toolResult }) => toolResult),
          }),
        }));
        const stats = { charsBefore: 4, charsAfter: 2, calls: 2, requests: 1, elapsedMs: 1 };
        return { ok: true, text: JSON.stringify({ messages: kept, stats }) };
      },
    },
    ui: { log: () => {}, toast: () => {} },
    clock: { now: async () => 0, after: () => {} },
  };

  const output = await handlers.get('session.compact')($, { messages }, () => 'next');

  assert.ok(!JSON.stringify(posted).includes('stdout'));
  assert.ok(posted.every((message) => message.toolUses.every((use) => !('text' in use))));
  assert.equal(posted[1].toolUses[0].agentId, 'a');
  assert.deepEqual(output.messages, [messages[0], messages[3], messages[4]]);
  assert.deepEqual(restore(strip(messages), messages), messages);
});

test('auto-compaction is due from the ceiling, or from the floor when two turns at the recent pace reach it', () => {
  const config = { compactAtPercent: 50, ceilingPercent: 80 };
  assert.equal(due(80, [], config), true, 'the ceiling always compacts');
  assert.equal(due(49, [20], config), false, 'never below the floor');
  assert.equal(due(50, [], config), false, 'no pace yet');
  assert.equal(due(56, [12], config), true, 'fast growth: 56 + 2×12 reaches 80');
  assert.equal(due(70, [2, 3, 4], config), false, 'slow growth waits: 70 + 2×3 < 80');
  assert.equal(due(74, [2, 3, 4], config), true);
});

test('auto-compaction tracks per-turn growth and waits for meaningful growth before retrying', async () => {
  const handlers = new Map();
  register((name, handler) => handlers.set(name, handler), {});
  let percent = 30;
  let calls = 0;
  const context = {
    session: {
      usage: async () => ({ context: { percent } }),
      compact: async () => { calls += 1; },
    },
    ui: { log: () => {} },
  };
  const complete = async (at) => {
    percent = at;
    return handlers.get('turn.complete')(context, {}, () => 'next');
  };

  assert.equal(await complete(30), 'next');
  await complete(42);
  assert.equal(calls, 0, 'below the floor');
  await complete(56);
  assert.equal(calls, 1, '56 + 2×13 reaches 80');
  await complete(62);
  assert.equal(calls, 1, 'due again, but the 10-point retry gap holds it');
  await complete(67);
  assert.equal(calls, 2);
  await complete(20);
  await complete(81);
  assert.equal(calls, 3, 'the ceiling compacts at once after a drop');
});

// A fake `$` with a clock that moves 700 ms per compaction, no band state (so the notice
// toasts), and the given fetch and toast.
function engine(fetch, toast = () => {}) {
  let now = 0;
  const toasts = [];
  const $ = {
    http: { fetch: async (...args) => { now += 700; return fetch(...args); } },
    clock: { now: async () => now, after: () => {} },
    ui: { log: () => { throw new Error('log down'); }, toast: (text) => { toasts.push(text); toast(); } },
  };
  return { $, toasts };
}

test('each compaction toasts once as `<diamond> <what> · <outcome> · <elapsed>`, without server text, and never throws', async () => {
  const handlers = new Map();
  register((name, handler) => handlers.set(name, handler), {});
  const messages = [{ role: 'user', text: 'secret prompt', toolUses: [] }];
  const toastsFor = async (fetch, toast, event = { messages }) => {
    const { $, toasts } = engine(fetch, toast);
    assert.equal(await handlers.get('session.compact')($, event, () => 'next') !== undefined, true);
    return toasts;
  };
  const stats = { charsBefore: 100, charsAfter: 26, calls: 1, requests: 1, elapsedMs: 700 };
  const ok = async () => ({ ok: true, text: JSON.stringify({ messages, stats }) });

  assert.deepEqual(await toastsFor(ok), ['◆ compact · applied 74% · 0.7s']);
  assert.deepEqual(
    await toastsFor(async () => ({ ok: true, text: JSON.stringify({ messages, stats: { ...stats, charsAfter: 83 } }) })),
    ['◊ compact · fallback: reduction 17% below 25% · 0.7s'],
  );
  assert.deepEqual(
    await toastsFor(async () => ({ ok: false, status: 413, text: 'secret prompt' })),
    ['◊ compact · fallback: HTTP 413 · 0.7s'],
  );
  assert.deepEqual(
    await toastsFor(ok, () => { throw new Error('toast down'); }),
    ['◆ compact · applied 74% · 0.7s'],
  );
  const asked = async () => ({ ok: true, text: JSON.stringify({ messages, stats: { ...stats, requestIds: ['req_1', 'req_2'] } }) });
  assert.deepEqual(
    await toastsFor(asked, undefined, { messages, agentId: 'a9f3c2' }),
    ['◆ compact · applied 74% · jev req_1 · agent a9f3c2 · 0.7s'],
    "the first Jev request's id, then a subagent's id",
  );
});

test('exhausted usage toasts once as failed, then compaction stays with the built-in', async () => {
  const handlers = new Map();
  register((name, handler) => handlers.set(name, handler), {});
  const { $, toasts } = engine(async () => ({ ok: false, status: 429, text: JSON.stringify({ disableHooks: true }) }));
  for (let i = 0; i < 2; i += 1) await handlers.get('session.compact')($, { messages: [] }, () => 'next');
  assert.deepEqual(toasts, ['󱇎 compact · disabled: TypeSafe usage exhausted · 0.7s']);
});
