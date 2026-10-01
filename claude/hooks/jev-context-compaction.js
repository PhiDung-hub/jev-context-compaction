import { FRESH, draw, drawer, duration, fresh, merged, plain, push } from './band.js';

const defaults = {
  endpoint: 'http://127.0.0.1:8787/compact',
  compactAtPercent: 50,
  ceilingPercent: 80,
  autoRetryDeltaPercent: 10,
  minReductionRatio: 0.25,
};

function finite(options, key, fallback) {
  const value = options[key];
  return typeof value === 'number' && Number.isFinite(value) ? value : fallback;
}

function configFrom(options) {
  const compact = {};
  for (const key of [
    'keepBudgetRatio',
    'preserveRecentMessages',
    'maxStateTokens',
    'maxCombinedTokens',
    'maxStateQuestionTokens',
    'maxParallelRequests',
    'requestTimeoutMs',
    'truncateHeadChars',
  ]) {
    const value = options[key];
    if (typeof value === 'number' && Number.isFinite(value)) compact[key] = value;
  }
  if (typeof options.model === 'string' && options.model) compact.model = options.model;
  return {
    endpoint:
      typeof options.endpoint === 'string' && options.endpoint
        ? options.endpoint
        : defaults.endpoint,
    compactAtPercent: finite(
      options,
      'compactAtPercent',
      defaults.compactAtPercent,
    ),
    ceilingPercent: finite(options, 'ceilingPercent', defaults.ceilingPercent),
    autoRetryDeltaPercent: Math.max(1, finite(
      options,
      'autoRetryDeltaPercent',
      defaults.autoRetryDeltaPercent,
    )),
    minReductionRatio: finite(
      options,
      'minReductionRatio',
      defaults.minReductionRatio,
    ),
    compact,
  };
}

function reduction(result) {
  const before = result.stats.charsBefore;
  return before === 0 ? 0 : Math.max(0, before - result.stats.charsAfter) / before;
}

function summary(result) {
  const stats = result.stats;
  return `${Math.round(reduction(result) * 100)}% reduction; ${stats.calls} calls; ${stats.requests} Jev request(s); ${stats.elapsedMs} ms`;
}

function byId(messages, key) {
  return new Map(messages.flatMap((message) => message[key] ?? []).map((item) => [item.tool_use_id, item]));
}

// Claude Code attaches each result to its call (text + structured `result`); the
// compactor reads neither, so send one copy and restore the rest afterwards.
function strip(messages) {
  const results = byId(messages, 'toolResults');
  return messages.map((message) => ({
    ...message,
    toolUses: message.toolUses.map(({ result, text, ...use }) =>
      text === undefined || text === results.get(use.tool_use_id)?.text ? use : { ...use, text }),
    ...(message.toolResults && {
      toolResults: message.toolResults.map(({ result, ...toolResult }) => toolResult),
    }),
  }));
}

function restore(compacted, original) {
  const uses = byId(original, 'toolUses');
  const results = byId(original, 'toolResults');
  return compacted.map((message) => ({
    ...message,
    toolUses: message.toolUses.map((use) => ({ ...uses.get(use.tool_use_id), ...use })),
    ...(message.toolResults && {
      toolResults: message.toolResults.map((toolResult) => ({
        ...results.get(toolResult.tool_use_id),
        ...toolResult,
      })),
    }),
  }));
}

function log($, text) {
  try {
    $.ui.log(`jev-context-compaction ${text}`);
  } catch {}
}

// The shared band's lines (types/index.d.ts), each peer's own, in band.js PEERS order;
// the session's, so a reload keeps them. This pack writes BAND alone.
const BAND = { plugin: 'jev-context-compaction', key: 'band' };
const PEER = { plugin: 'jev-input-standardizer', key: 'band' };
// Set by Agent OS when its band draws these notices; the packs then draw none.
const AGENT_OS = { plugin: 'agent-os', key: 'drawsNotices' };

// Whether this session draws the band: its last AbovePrompt held no survey.
let banded = false;

// Writes the band through `change`, again when another write came first.
async function edit($, change) {
  for (let tries = 0; tries < 3; tries += 1) {
    const { value = [], version } = await $.state.get(BAND);
    const { isSet } = await $.state.set(BAND, change(value), { ifVersion: version });
    if (isSet) return;
  }
}

// The notice joins the band (band.js), replacing the line under `key`; it leaves after
// FRESH, a running line once its outcome replaces it. An outcome is a toast where no band
// draws; a running or hint line never is. Never throws.
async function show($, notice, key) {
  const quiet = notice.outcome === 'running' || notice.outcome === 'hint';
  let drawn = false;
  try {
    const now = await $.clock.now();
    await edit($, (lines) => push(lines, notice, now, key));
    if (notice.outcome !== 'running') {
      $.clock.after(FRESH + 50, async () => {
        try {
          const later = await $.clock.now();
          await edit($, (lines) => fresh(lines, later));
        } catch {}
      });
    }
    drawn = banded;
  } catch {}
  if (drawn || quiet) return;
  try {
    $.ui.toast(plain(notice), { timeoutMs: FRESH });
  } catch {}
}

// Another plugin's value reads as unset when that plugin is absent or refuses the read,
// so a missing peer never takes the band down.
async function agentOsDraws($) {
  try {
    return (await $.state.get(AGENT_OS)).value === true;
  } catch {
    return false;
  }
}

async function peerLines($) {
  try {
    return (await $.state.get(PEER)).value ?? [];
  } catch {
    return [];
  }
}

// The shared band: every peer's fresh lines above what the chain beneath drew, drawn by
// the first peer holding one (band.js `drawer`), or that drawing alone while a survey
// holds it, Agent OS draws the notices, or nothing is fresh.
async function band($, e, next) {
  banded = e?.props?.hasSurvey === false;
  let lines = [];
  let now = 0;
  try {
    if (banded && !(await agentOsDraws($))) {
      const bands = [(await $.state.get(BAND)).value ?? [], await peerLines($)];
      now = await $.clock.now();
      if (drawer(bands, now) === 0) lines = merged(bands, now);
    }
  } catch {
    banded = false;
  }
  const below = await next(e);
  return lines.length ? draw($.ui.resolve(e), lines, below, now) : below;
}

// One notice per action (band.js), drawn above the prompt or else toasted; it never
// carries payloads or server text: that detail goes to the log only.
function notify($, notice, key, detail) {
  log($, detail);
  return show($, notice, key);
}

/** Whether auto-compaction is due at `percent` full, given the last turns' growth in
 * points: always from the ceiling, and from the floor once about two more turns at the
 * recent pace would reach the ceiling. */
function due(percent, growth, { compactAtPercent, ceilingPercent }) {
  if (percent >= ceilingPercent) return true;
  if (percent < compactAtPercent || growth.length === 0) return false;
  const pace = growth.reduce((sum, points) => sum + points, 0) / growth.length;
  return percent + 2 * pace >= ceilingPercent;
}

async function run($, messages, config) {
  const response = await $.http.fetch(config.endpoint, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ messages, options: config.compact }),
  });
  if (!response.ok) {
    let signal;
    try {
      signal = JSON.parse(response.text);
    } catch {
      signal = {};
    }
    const error = new Error(
      `local compactor HTTP ${response.status}: ${signal.message ?? response.text}`,
    );
    error.status = response.status;
    error.disableHooks = signal.disableHooks === true;
    throw error;
  }
  return JSON.parse(response.text);
}

/** @type {import('claude-code').Register} */
export const register = (on, options) => {
  banded = false;
  const config = configFrom(options);
  let compacting = false;
  let disabled = false;
  let lastAutoAttemptPercent = null;
  // The context fill after the last turn, and how many points each of the last three
  // growing turns added.
  let lastPercent = null;
  const growth = [];

  on('session.compact', async ($, event, next) => {
    if (disabled) return next(event);
    log($, 'started');
    // The subagent whose transcript compacts; none for the main conversation.
    const agent = event.agentId ? [{ kind: 'agent', id: event.agentId }] : [];
    const key = `compact:${event.agentId ?? 'main'}`;
    const started = await $.clock.now();
    const took = async () => duration((await $.clock.now()) - started);
    // The running line counts up until the outcome replaces it.
    await show($, { outcome: 'running', what: 'compact', status: 'running', refs: agent, since: started }, key);
    let tick;
    try {
      tick = $.clock.every(250, () => $.ui.invalidate('ui.render'));
    } catch {}
    try {
      const result = await run($, strip(event.messages), config);
      const percent = Math.round(reduction(result) * 100);
      // The first Jev request's id; a compaction that asked Jev nothing has none.
      const jev = result.stats.requestIds?.[0];
      const refs = [...(jev ? [{ kind: 'jev', id: jev }] : []), ...agent];
      if (reduction(result) < config.minReductionRatio) {
        const status = `fallback: reduction ${percent}% below ${Math.round(config.minReductionRatio * 100)}%`;
        await notify($, { outcome: 'look', what: 'compact', status, refs, elapsed: await took() }, key,
          `built-in fallback below reduction threshold (${summary(result)})`);
        return next(event);
      }
      await notify($, { outcome: 'done', what: 'compact', status: `applied ${percent}%`, refs, elapsed: await took() }, key,
        `applied (${summary(result)})`);
      return { messages: restore(result.messages, event.messages) };
    } catch (error) {
      const elapsed = await took();
      if (error instanceof Error && error.disableHooks === true) {
        disabled = true;
        await notify($, { outcome: 'failed', what: 'compact', status: 'disabled: TypeSafe usage exhausted', refs: agent, elapsed }, key,
          'disabled: TypeSafe API usage is exhausted');
        return next(event);
      }
      const status = `fallback: ${error?.status ? `HTTP ${error.status}` : 'compactor error'}`;
      await notify($, { outcome: 'look', what: 'compact', status, refs: agent, elapsed }, key,
        `fallback (${error instanceof Error ? error.message : String(error)})`);
      return next(event);
    } finally {
      try {
        tick?.cancel();
      } catch {}
    }
  });

  on('turn.complete', async ($, event, next) => {
    if (disabled || compacting) return next(event);
    try {
      const { context } = await $.session.usage();
      const percent = context.percent;
      if (!Number.isFinite(percent)) return next(event);
      if (lastPercent !== null && percent > lastPercent) growth.push(percent - lastPercent);
      if (growth.length > 3) growth.shift();
      lastPercent = percent;
      if (percent < config.compactAtPercent) lastAutoAttemptPercent = null;
      if (!due(percent, growth, config)) return next(event);
      if (
        lastAutoAttemptPercent !== null &&
        percent < lastAutoAttemptPercent + config.autoRetryDeltaPercent
      ) return next(event);
      lastAutoAttemptPercent = percent;
      compacting = true;
      await $.session.compact();
    } catch (error) {
      log($, `auto-compact skipped (${error instanceof Error ? error.message : String(error)})`);
    } finally {
      compacting = false;
    }
    return next(event);
  });

  on('ui.render', { component: 'AbovePrompt' }, band);
};

export { configFrom, due, reduction, restore, strip, summary };
