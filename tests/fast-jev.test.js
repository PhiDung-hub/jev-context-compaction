import assert from 'node:assert/strict';
import test from 'node:test';

import { register, restore, strip } from '../claude/hooks/jev-context-compaction.js';

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
  };

  const output = await handlers.get('session.compact')($, { messages }, () => 'next');

  assert.ok(!JSON.stringify(posted).includes('stdout'));
  assert.ok(posted.every((message) => message.toolUses.every((use) => !('text' in use))));
  assert.equal(posted[1].toolUses[0].agentId, 'a');
  assert.deepEqual(output.messages, [messages[0], messages[3], messages[4]]);
  assert.deepEqual(restore(strip(messages), messages), messages);
});

test('auto-compaction waits for meaningful usage growth before retrying', async () => {
  const handlers = new Map();
  register((name, handler) => handlers.set(name, handler), {});
  let percent = 60;
  let calls = 0;
  const context = {
    session: {
      usage: async () => ({ context: { percent } }),
      compact: async () => { calls += 1; },
    },
    ui: { log: () => {} },
  };
  const complete = async () => handlers.get('turn.complete')(context, {}, () => 'next');

  assert.equal(await complete(), 'next');
  percent = 65;
  assert.equal(await complete(), 'next');
  assert.equal(calls, 1);
  percent = 70;
  await complete();
  assert.equal(calls, 2);
  percent = 20;
  await complete();
  percent = 60;
  await complete();
  assert.equal(calls, 3);
});

test('each compaction toasts once as `<what> · <outcome>`, without server text, and never throws', async () => {
  const handlers = new Map();
  register((name, handler) => handlers.set(name, handler), {});
  const messages = [{ role: 'user', text: 'secret prompt', toolUses: [] }];
  const toastsFor = async (fetch, toast = () => {}) => {
    const toasts = [];
    const $ = {
      http: { fetch },
      ui: { log: () => { throw new Error('log down'); }, toast: (text) => { toasts.push(text); toast(); } },
    };
    assert.equal(await handlers.get('session.compact')($, { messages }, () => 'next') !== undefined, true);
    return toasts;
  };
  const stats = { charsBefore: 100, charsAfter: 26, calls: 1, requests: 1, elapsedMs: 700 };

  assert.deepEqual(
    await toastsFor(async () => ({ ok: true, text: JSON.stringify({ messages, stats }) })),
    ['compact · applied 74% in 0.7 s'],
  );
  assert.deepEqual(
    await toastsFor(async () => ({ ok: false, status: 413, text: 'secret prompt' })),
    ['fallback · HTTP 413'],
  );
  assert.deepEqual(
    await toastsFor(async () => ({ ok: true, text: JSON.stringify({ messages, stats }) }), () => {
      throw new Error('toast down');
    }),
    ['compact · applied 74% in 0.7 s'],
  );
});
