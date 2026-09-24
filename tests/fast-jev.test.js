import assert from 'node:assert/strict';
import test from 'node:test';

import { register } from '../claude/hooks/jev-context-compaction.js';

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
