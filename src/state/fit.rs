use crate::model::{CompactOptions, CompactionError, Message, Result, ToolCall};

use super::FittedState;
use super::representation::{
    abridge, compact_old_calls, goal_from_messages, history_entries, input_stage, is_pinned,
    merge_call_runs, shrink_order, state, state_tokens, within,
};

const INPUT_LIMITS: [usize; 3] = [1_000, 200, 60];
const TEXT_HEAD: usize = 400;
const TEXT_TAIL: usize = 150;

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
    for limit in INPUT_LIMITS {
        let state = state(goal.clone(), history_entries(messages, calls, limit));
        if let Some(fitted) = within(state, options.max_state_tokens, input_stage(limit))? {
            return Ok(fitted);
        }
    }

    let mut history = history_entries(messages, calls, INPUT_LIMITS[2]);
    let order = shrink_order(&history, messages.len(), options.preserve_recent_messages);
    for index in &order {
        let text = &history[*index].text;
        if text.len() > TEXT_HEAD + TEXT_TAIL + 40 {
            history[*index].text = abridge(text, TEXT_HEAD, TEXT_TAIL);
            if let Some(fitted) = within(
                state(goal.clone(), history.clone()),
                options.max_state_tokens,
                "texts abridged",
            )? {
                return Ok(fitted);
            }
        }
    }
    for index in &order {
        let entry = &mut history[*index];
        if !is_pinned(entry.i, messages.len(), options.preserve_recent_messages)
            && !entry.text.is_empty()
        {
            entry.text = format!("[… {} chars omitted …]", messages[entry.i].text.len());
            if let Some(fitted) = within(
                state(goal.clone(), history.clone()),
                options.max_state_tokens,
                "old messages collapsed",
            )? {
                return Ok(fitted);
            }
        }
    }
    compact_old_calls(
        &mut history,
        calls,
        messages.len(),
        options.preserve_recent_messages,
    );
    if let Some(fitted) = within(
        state(goal.clone(), history.clone()),
        options.max_state_tokens,
        "old calls compacted",
    )? {
        return Ok(fitted);
    }
    history.retain(|entry| {
        is_pinned(entry.i, messages.len(), options.preserve_recent_messages)
            || !entry.tool_calls.is_empty()
    });
    merge_call_runs(
        &mut history,
        messages.len(),
        options.preserve_recent_messages,
    );
    let state = state(goal, history);
    let tokens = state_tokens(&state)?;
    if tokens <= options.max_state_tokens {
        return Ok(FittedState {
            state,
            tokens,
            stage: "old calls merged".to_owned(),
        });
    }
    Err(CompactionError::StateTooLarge(format!(
        "estimated {tokens} tokens after fitting; limit is {}",
        options.max_state_tokens
    )))
}
