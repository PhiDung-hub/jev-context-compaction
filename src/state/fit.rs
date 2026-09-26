use crate::model::{CompactOptions, CompactionError, Message, Result, ToolCall};

use super::representation::{
    abridge, compact_old_calls, drop_head, goal_from_messages, head_slots, history_entries,
    input_stage, is_pinned, keep_newest_heads, merge_call_runs, newest_heads_within, shrink_order,
    state, state_tokens,
};
use super::{FittedState, HistoryEntry};
use crate::tokens::{estimate_token_tenths, tenths_to_tokens};

const INPUT_LIMITS: [usize; 3] = [1_000, 200, 60];
const TEXT_HEAD: usize = 400;
const TEXT_TAIL: usize = 150;

/// Share of the state budget, in percent, held for the newest result heads while old text shrinks.
const HEAD_SHARE_PERCENT: usize = 20;

/// Fit a transcript-derived state into the configured model budget.
///
/// # Errors
///
/// Returns an error when even the most compact representation is too large.
pub fn fit_state(
    messages: &[Message],
    calls: &[ToolCall],
    options: &CompactOptions,
) -> Result<FittedState> {
    let goal = if options.goal.is_empty() {
        goal_from_messages(messages)
    } else {
        options.goal.clone()
    };
    let share = options.max_state_tokens * HEAD_SHARE_PERCENT / 100;
    match fit_with_head_share(messages, calls, options, &goal, share) {
        Err(CompactionError::StateTooLarge(_)) if share > 0 => {
            fit_with_head_share(messages, calls, options, &goal, 0)
        }
        fitted => fitted,
    }
}

/// Heads beyond the newest `share` tokens go first at each input limit, before any text
/// shrinks; the newest stay through every later stage.
fn fit_with_head_share(
    messages: &[Message],
    calls: &[ToolCall],
    options: &CompactOptions,
    goal: &str,
    share: usize,
) -> Result<FittedState> {
    let limit = options.max_state_tokens;
    let (total, recent) = (messages.len(), options.preserve_recent_messages);
    let mut keep = 0;
    for input in INPUT_LIMITS {
        let history = history_entries(messages, calls, input, options.truncate_head_chars);
        keep = newest_heads_within(&history, share)?;
        if let Some(fitted) = fit_dropping_heads(goal, history, limit, keep, input_stage(input))? {
            return Ok(fitted);
        }
    }

    let mut history = history_entries(
        messages,
        calls,
        INPUT_LIMITS[2],
        options.truncate_head_chars,
    );
    keep_newest_heads(&mut history, keep);
    let mut size = StateSize::new(goal, &history)?;
    let order = shrink_order(&history, total, recent);
    for index in &order {
        let text = &history[*index].text;
        if text.len() > TEXT_HEAD + TEXT_TAIL + 40 {
            history[*index].text = abridge(text, TEXT_HEAD, TEXT_TAIL);
            if size.update(&history, *index)? <= limit {
                return Ok(size.fitted(goal, history, &with_heads("texts abridged", keep)));
            }
        }
    }
    for index in &order {
        let entry = &mut history[*index];
        if !is_pinned(entry.i, total, recent) && !entry.text.is_empty() {
            entry.text = format!("[… {} chars omitted …]", messages[entry.i].text.len());
            if size.update(&history, *index)? <= limit {
                let stage = with_heads("old messages collapsed", keep);
                return Ok(size.fitted(goal, history, &stage));
            }
        }
    }
    compact_old_calls(&mut history, calls, total, recent);
    let size = StateSize::new(goal, &history)?;
    if size.tokens() <= limit {
        return Ok(size.fitted(goal, history, &with_heads("old calls compacted", keep)));
    }
    history.retain(|entry| is_pinned(entry.i, total, recent) || !entry.tool_calls.is_empty());
    merge_call_runs(&mut history, total, recent);
    let state = state(goal.to_owned(), history);
    let tokens = state_tokens(&state)?;
    if tokens <= limit {
        return Ok(FittedState {
            state,
            tokens,
            stage: with_heads("old calls merged", keep),
        });
    }
    Err(CompactionError::StateTooLarge(format!(
        "estimated {tokens} tokens after fitting; limit is {limit}"
    )))
}

/// Drop result heads oldest first, one at a time, until the state fits, never the newest
/// `keep`; `None` if it never does.
fn fit_dropping_heads(
    goal: &str,
    mut history: Vec<HistoryEntry>,
    limit: usize,
    keep: usize,
    stage: &str,
) -> Result<Option<FittedState>> {
    let mut size = StateSize::new(goal, &history)?;
    let slots = head_slots(&history);
    let mut kept = slots.len();
    for (entry, call) in slots.into_iter().take(kept.saturating_sub(keep)) {
        if size.tokens() <= limit {
            break;
        }
        drop_head(&mut history[entry], call);
        size.update(&history, entry)?;
        kept -= 1;
    }
    if size.tokens() > limit {
        return Ok(None);
    }
    Ok(Some(size.fitted(goal, history, &with_heads(stage, kept))))
}

/// `stage` or `stage, N heads`.
fn with_heads(stage: &str, heads: usize) -> String {
    if heads == 0 {
        stage.to_owned()
    } else {
        format!("{stage}, {heads} heads")
    }
}

/// Estimated state size, kept exact per history edit without re-serialising the state.
///
/// The estimate only counts runs of letters or digits, and JSON punctuation separates the
/// serialised entries, so the state's tenths are the sum of its parts.
struct StateSize {
    entries: Vec<usize>,
    tenths: usize,
}

impl StateSize {
    fn new(goal: &str, history: &[HistoryEntry]) -> Result<Self> {
        let empty = serde_json::to_string(&state(goal.to_owned(), Vec::new()))?;
        let entries = history
            .iter()
            .map(entry_tenths)
            .collect::<Result<Vec<_>>>()?;
        let commas = estimate_token_tenths(",") * entries.len().saturating_sub(1);
        let tenths = estimate_token_tenths(&empty) + commas + entries.iter().sum::<usize>();
        Ok(Self { entries, tenths })
    }

    fn update(&mut self, history: &[HistoryEntry], index: usize) -> Result<usize> {
        let tenths = entry_tenths(&history[index])?;
        self.tenths = self.tenths - self.entries[index] + tenths;
        self.entries[index] = tenths;
        Ok(self.tokens())
    }

    fn tokens(&self) -> usize {
        tenths_to_tokens(self.tenths)
    }

    fn fitted(&self, goal: &str, history: Vec<HistoryEntry>, stage: &str) -> FittedState {
        FittedState {
            state: state(goal.to_owned(), history),
            tokens: self.tokens(),
            stage: stage.to_owned(),
        }
    }
}

fn entry_tenths(entry: &HistoryEntry) -> Result<usize> {
    Ok(estimate_token_tenths(&serde_json::to_string(entry)?))
}

#[cfg(test)]
mod tests;
