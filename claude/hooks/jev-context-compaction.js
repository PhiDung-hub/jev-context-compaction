const defaults = {
  endpoint: 'http://127.0.0.1:8787/compact',
  compactAtPercent: 60,
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
    'keepThreshold',
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

function notify($, text) {
  $.ui.log(text);
  $.ui.toast(text, { timeoutMs: 15000 });
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
    error.disableHooks = signal.disableHooks === true;
    throw error;
  }
  return JSON.parse(response.text);
}

/** @type {import('claude-code').Register} */
export const register = (on, options) => {
  const config = configFrom(options);
  let compacting = false;
  let disabled = false;
  let lastAutoAttemptPercent = null;

  on('session.compact', async ($, event, next) => {
    if (disabled) return next(event);
    notify($, 'jev-context-compaction started');
    try {
      const result = await run($, event.messages, config);
      if (reduction(result) < config.minReductionRatio) {
        notify($, `jev-context-compaction finished; built-in fallback below reduction threshold (${summary(result)})`);
        return next(event);
      }
      notify($, `jev-context-compaction applied (${summary(result)})`);
      return { messages: result.messages };
    } catch (error) {
      if (error instanceof Error && error.disableHooks === true) {
        disabled = true;
        notify($, 'jev-context-compaction disabled: TypeSafe API usage is exhausted');
        return next(event);
      }
      notify($, `jev-context-compaction fallback (${error instanceof Error ? error.message : String(error)})`);
      return next(event);
    }
  });

  on('turn.complete', async ($, event, next) => {
    if (disabled || compacting) return next(event);
    try {
      const { context } = await $.session.usage();
      const percent = context.percent;
      if (!Number.isFinite(percent)) return next(event);
      if (percent < config.compactAtPercent) {
        lastAutoAttemptPercent = null;
        return next(event);
      }
      if (
        lastAutoAttemptPercent !== null &&
        percent < lastAutoAttemptPercent + config.autoRetryDeltaPercent
      ) return next(event);
      lastAutoAttemptPercent = percent;
      compacting = true;
      await $.session.compact();
    } catch (error) {
      $.ui.log(`jev-context-compaction auto-compact skipped (${error instanceof Error ? error.message : String(error)})`);
    } finally {
      compacting = false;
    }
    return next(event);
  });
};

export { configFrom, reduction, summary };
