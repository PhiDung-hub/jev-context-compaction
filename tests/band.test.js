import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import test from 'node:test';

import { FRESH, PALETTE, draw, drawer, duration, fresh, merged, plain, push, segments } from '../claude/hooks/band.js';

const NOTICE = { outcome: 'done', what: 'compact', status: 'applied 74%', refs: [{ kind: 'jev', id: 'req_7f3a' }, { kind: 'agent', id: 'a1b2' }], elapsed: '0.7s' };

test('each outcome opens with its diamond, the status coloured the same', () => {
  const cases = { done: ['◆', PALETTE.green], unchanged: ['󰜌', PALETTE.dim], look: ['◊', PALETTE.yellow], failed: ['󱇎', PALETTE.red] };
  for (const [outcome, [glyph, color]] of Object.entries(cases)) {
    const parts = segments({ outcome, what: 'x', status: 'y' });
    assert.deepEqual(parts[0], { text: `${glyph} `, color });
    assert.deepEqual(parts[3], { text: 'y', color });
  }
});

test('the ref id is blue and underlined, the elapsed time magenta, and absent ones add nothing', () => {
  assert.deepEqual(segments(NOTICE).slice(4), [
    { text: ' · jev ' },
    { text: 'req_7f3a', color: PALETTE.blue, underline: true },
    { text: ' · agent ' },
    { text: 'a1b2', color: PALETTE.blue, underline: true },
    { text: ' · ' },
    { text: '0.7s', color: PALETTE.magenta },
  ]);
  assert.equal(segments({ outcome: 'look', what: 'fallback', status: 'HTTP 413' }).length, 4);
  assert.equal(segments({ ...NOTICE, refs: undefined }).at(-1).color, PALETTE.magenta);
});

test('the toast is the same line uncoloured, with the diamond', () => {
  assert.equal(plain(NOTICE), '◆ compact · applied 74% · jev req_7f3a · agent a1b2 · 0.7s');
  assert.equal(plain({ outcome: 'failed', what: 'fallback', status: 'disabled' }), '󱇎 fallback · disabled');
});

test('durations read as seconds, then minutes', () => {
  assert.deepEqual([700, 9_949, 42_300, 185_000, -5].map(duration), ['0.7s', '9.9s', '42s', '3m05s', '0.0s']);
});

test('the band keeps the newest three fresh lines', () => {
  let lines = [];
  for (let i = 0; i < 4; i += 1) lines = push(lines, { ...NOTICE, status: `n${i}` }, i);
  assert.deepEqual(lines.map((line) => line.notice.status), ['n1', 'n2', 'n3']);
  assert.deepEqual(push(lines, NOTICE, 3 + FRESH).map((line) => line.notice.status), ['applied 74%']);
});

test('the band draws one row per line above what the chain beneath drew', () => {
  const elements = { Box: (props) => ({ type: 'Box', props }), Text: (props) => ({ type: 'Text', props }) };
  const below = { type: 'engine', ref: 0 };
  const tree = draw(elements, push([], NOTICE, 0), below);
  assert.equal(tree.props.children.at(-1), below);
  const row = tree.props.children[0].props.children;
  assert.deepEqual(row.map((text) => text.props.children), segments(NOTICE).map((part) => part.text));
  assert.deepEqual(row[5].props, { color: PALETTE.blue, underline: true, children: 'req_7f3a' });
  assert.deepEqual(row[2].props, { children: ' · ' }, 'no undefined props');
});

// Each pack ships its own copy; in this repo both must stay identical.
const SIBLINGS = ['jev-context-compaction', 'jev-input-standardizer'].map((pack) => new URL(`../../${pack}/claude/hooks/band.js`, import.meta.url));
test('both packs ship the same band.js', { skip: !SIBLINGS.every(existsSync) && 'published alone' }, () => {
  assert.equal(readFileSync(SIBLINGS[0], 'utf8'), readFileSync(SIBLINGS[1], 'utf8'));
});

test('a running line counts up from `since`, never toasts its own time, and its outcome replaces it by key', () => {
  const running = { outcome: 'running', what: 'compact', status: 'running', since: 1_000 };
  assert.deepEqual(segments(running, 1_800).slice(-1), [{ text: '0.8s', color: PALETTE.magenta }]);
  assert.equal(segments(running, 1_800)[0].text, '◌ ');
  let lines = push([], running, 1_000, 'compact:main');
  lines = push(lines, { ...NOTICE, refs: undefined }, 2_000, 'compact:main');
  assert.deepEqual(lines.map((line) => line.notice.outcome), ['done']);
});

test('a running line outlives FRESH until replaced; an outcome does not', () => {
  const lines = push(push([], { outcome: 'running', what: 'compact', status: 'running', since: 0 }, 0, 'a'), NOTICE, 0);
  assert.deepEqual(fresh(lines, FRESH + 1).map((line) => line.notice.outcome), ['running']);
});

test('the first peer holding a fresh line draws every peer’s lines, oldest first', () => {
  const ours = push([], { ...NOTICE, status: 'ours' }, 5);
  const theirs = push(push([], { ...NOTICE, status: 'old' }, 1), { ...NOTICE, status: 'new' }, 9);
  assert.equal(drawer([ours, theirs], 10), 0);
  assert.equal(drawer([[], theirs], 10), 1);
  assert.equal(drawer([[], []], 10), -1);
  assert.deepEqual(merged([ours, theirs], 10).map((line) => line.notice.status), ['old', 'ours', 'new']);
  assert.deepEqual(merged([ours, theirs], 9 + FRESH).map((line) => line.notice.status), []);
});
