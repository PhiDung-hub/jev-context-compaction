// Outcome notices in a band above the prompt, as the Agent OS band draws its tags: the
// outcome diamond and status coloured by outcome, the action cyan, a ref id blue and
// underlined, the elapsed time magenta. Where no band draws, the same line is a toast.
// The packs share one band: each keeps its own lines, and the first of PEERS with a fresh
// line draws every peer's lines, oldest first (`drawer`, `merged`).
// This file is the model and the drawing; the hooks module feeds it (Claude Code follows
// `$` only within one file). Each pack ships an identical copy: a pack is published alone.

// Phil's WezTerm palette (Tokyo Night).
export const PALETTE = {
  green: '#9ece6a',
  yellow: '#e0af68',
  red: '#f7768e',
  dim: '#6f7584',
  blue: '#7aa2f7',
  cyan: '#7dcfff',
  magenta: '#bb9af7',
};
// A line stays as long as its toast would; a running line until its outcome replaces it,
// at most RUNNING.
export const FRESH = 15_000;
const RUNNING = 10 * 60_000;
const MAX_LINES = 3;
/** The packs sharing the band, in drawing priority; each hooks module reads them in
 * this order. */
export const PEERS = ['jev-context-compaction', 'jev-input-standardizer'];

/** Each outcome's diamond and colour: done, no change, needs a look, failed. All four are
 * in SauceCodePro, as the WezTerm status line draws them; the Nerd Font icons are
 * md-rhombus_outline and md-alert_rhombus. */
export const OUTCOMES = {
  done: { glyph: '◆', color: PALETTE.green },
  unchanged: { glyph: '\u{f070c}', color: PALETTE.dim },
  look: { glyph: '◊', color: PALETTE.yellow },
  failed: { glyph: '\u{f11ce}', color: PALETTE.red },
  // In flight, its time counting up; never a toast.
  running: { glyph: '◌', color: PALETTE.yellow },
  // A suggestion, never a toast.
  hint: { glyph: '○', color: PALETTE.dim },
};

/** `0.7s` under 10 s, `42s` under a minute, else `3m05s`. */
export function duration(ms) {
  const seconds = Math.max(0, ms) / 1000;
  if (seconds < 10) return `${seconds.toFixed(1)}s`;
  if (seconds < 60) return `${Math.round(seconds)}s`;
  const whole = Math.round(seconds);
  return `${Math.floor(whole / 60)}m${String(whole % 60).padStart(2, '0')}s`;
}

/** A notice `{ outcome, what, status, refs?: [{ kind, id }], elapsed?, since? }` as its
 * parts, `{ text, color?, underline? }`, joined by ` · `; `since` counts up to `now`. */
export function segments(notice, now) {
  const { glyph, color } = OUTCOMES[notice.outcome];
  const parts = [
    { text: `${glyph} `, color },
    { text: notice.what, color: PALETTE.cyan },
    { text: ' · ' },
    { text: notice.status, color },
  ];
  for (const ref of notice.refs ?? []) parts.push({ text: ` · ${ref.kind} ` }, { text: ref.id, color: PALETTE.blue, underline: true });
  const elapsed = notice.elapsed ?? (notice.since === undefined ? undefined : duration(now - notice.since));
  if (elapsed) parts.push({ text: ' · ' }, { text: elapsed, color: PALETTE.magenta });
  return parts;
}

/** The notice as a toast shows it: the same text, uncoloured. */
export function plain(notice) {
  return segments(notice).map((part) => part.text).join('');
}

/** Lines still shown at `now`. */
export function fresh(lines, now) {
  return lines.filter((line) => now - line.at < (line.notice.outcome === 'running' ? RUNNING : FRESH));
}

/** The band after `notice` arrives at `now`: the newest MAX_LINES fresh lines. A notice
 * with a `key` replaces the line holding that key (a running line, its outcome). */
export function push(lines, notice, now, key) {
  const kept = fresh(lines, now).filter((line) => key === undefined || line.key !== key);
  return [...kept, { at: now, notice, ...(key !== undefined && { key }) }].slice(-MAX_LINES);
}

/** Which peer draws the shared band: the first of `bands` (PEERS order) with a fresh
 * line, or -1 when none has one. */
export function drawer(bands, now) {
  return bands.findIndex((lines) => fresh(lines, now).length > 0);
}

/** Every peer's fresh lines, oldest first, the newest MAX_LINES. */
export function merged(bands, now) {
  return bands.flatMap((lines) => fresh(lines, now)).sort((a, b) => a.at - b.at).slice(-MAX_LINES);
}

/** The band's tree: one row per line, above whatever the plugins beneath drew. */
export function draw({ Box, Text }, lines, below, now) {
  const row = (line) => Box({ flexDirection: 'row', children: segments(line.notice, now).map((part) => Text({ ...style(part), children: part.text })) });
  return Box({ flexDirection: 'column', children: [...lines.map(row), below] });
}

// Only the props a part sets: a tree with an undefined prop is refused.
function style(part) {
  return Object.fromEntries(Object.entries({ color: part.color, underline: part.underline }).filter(([, v]) => v !== undefined));
}
