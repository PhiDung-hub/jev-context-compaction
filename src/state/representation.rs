use std::collections::BTreeMap;

use crate::model::{Message, Result, Role, ToolCall};
use crate::tokens::estimate_tokens;

use super::{CompactionState, FittedState, HistoryCall, HistoryEntry, STATE_CONTEXT};

pub(super) fn history_entries(
    messages: &[Message],
    calls: &[ToolCall],
    input_limit: usize,
) -> Vec<HistoryEntry> {
    let mut by_message = BTreeMap::<usize, Vec<&ToolCall>>::new();
    for call in calls {
        by_message.entry(call.call_index).or_default().push(call);
    }
    messages
        .iter()
        .enumerate()
        .filter_map(|(index, message)| {
            let tool_calls = by_message
                .get(&index)
                .into_iter()
                .flatten()
                .map(|call| HistoryCall::Structured {
                    id: call.id.clone(),
                    tool: call.tool.clone(),
                    input: truncate(
                        &serde_json::to_string(&call.input).unwrap_or_default(),
                        input_limit,
                    ),
                    result: result_note(call),
                })
                .collect::<Vec<_>>();
            (!message.text.trim().is_empty() || !tool_calls.is_empty()).then(|| HistoryEntry {
                i: index,
                role: message.role,
                text: message.text.clone(),
                tool_calls,
            })
        })
        .collect()
}

pub(super) fn state(goal: String, history: Vec<HistoryEntry>) -> CompactionState {
    CompactionState {
        context: STATE_CONTEXT,
        goal,
        history,
    }
}

pub(super) fn within(
    state: CompactionState,
    limit: usize,
    stage_label: &str,
) -> Result<Option<FittedState>> {
    let tokens = state_tokens(&state)?;
    Ok((tokens <= limit).then(|| FittedState {
        state,
        tokens,
        stage: stage_label.to_owned(),
    }))
}

pub(super) fn state_tokens(state: &CompactionState) -> Result<usize> {
    Ok(estimate_tokens(&serde_json::to_string(state)?))
}

pub(super) fn input_stage(limit: usize) -> &'static str {
    match limit {
        1_000 => "full",
        200 => "inputs<=200",
        _ => "inputs<=60",
    }
}

pub(super) fn is_pinned(index: usize, total: usize, recent: usize) -> bool {
    index == 0 || index >= total.saturating_sub(recent)
}

pub(super) fn goal_from_messages(messages: &[Message]) -> String {
    let prompts: Vec<_> = messages
        .iter()
        .filter(|message| {
            message.role == Role::User
                && !message.text.trim().is_empty()
                && message.tool_results.is_empty()
        })
        .rev()
        .take(3)
        .map(|message| truncate(&message.text, 500))
        .collect();
    prompts.into_iter().rev().collect::<Vec<_>>().join("\n")
}

fn result_note(call: &ToolCall) -> String {
    format!(
        "{}, {} chars omitted",
        if call.is_error { "error" } else { "ok" },
        call.result_chars
    )
}

fn compact_call(call: &ToolCall) -> String {
    let input = call
        .input
        .iter()
        .map(|(key, value)| {
            format!(
                "{key}={}",
                serde_json::to_string(value).unwrap_or_else(|_| "null".to_owned())
            )
        })
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "{} {} {} -> {} {}ch",
        call.id,
        call.tool,
        truncate(&input.replace('\n', " "), 60),
        if call.is_error { "error" } else { "ok" },
        call.result_chars
    )
}

pub(super) fn compact_old_calls(
    history: &mut [HistoryEntry],
    calls: &[ToolCall],
    total: usize,
    recent: usize,
) {
    for entry in history {
        if is_pinned(entry.i, total, recent) || entry.tool_calls.is_empty() {
            continue;
        }
        entry.tool_calls = calls
            .iter()
            .filter(|call| call.call_index == entry.i)
            .map(|call| HistoryCall::Compact(compact_call(call)))
            .collect();
    }
}

pub(super) fn merge_call_runs(history: &mut Vec<HistoryEntry>, total: usize, recent: usize) {
    let mut merged = Vec::<HistoryEntry>::new();
    for entry in history.drain(..) {
        let foldable = entry.text.is_empty()
            && !entry.tool_calls.is_empty()
            && !is_pinned(entry.i, total, recent);
        if foldable {
            if let Some(previous) = merged.last_mut() {
                if previous.role == entry.role
                    && previous.text.is_empty()
                    && !is_pinned(previous.i, total, recent)
                {
                    previous.tool_calls.extend(entry.tool_calls);
                    continue;
                }
            }
        }
        merged.push(entry);
    }
    *history = merged;
}

pub(super) fn shrink_order(history: &[HistoryEntry], total: usize, recent: usize) -> Vec<usize> {
    let mut order: Vec<_> = (0..history.len())
        .filter(|index| !is_pinned(history[*index].i, total, recent))
        .collect();
    order.extend((0..history.len()).filter(|index| is_pinned(history[*index].i, total, recent)));
    order
}

fn truncate(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut end = limit.saturating_sub(1).min(text.len());
    while !text.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    format!("{}…", &text[..end])
}

pub(super) fn abridge(text: &str, head: usize, tail: usize) -> String {
    if text.len() <= head + tail + 40 {
        return text.to_owned();
    }
    let mut head_end = head;
    while !text.is_char_boundary(head_end) {
        head_end -= 1;
    }
    let mut tail_start = text.len() - tail;
    while !text.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    format!(
        "{}\n[… {} chars omitted …]\n{}",
        &text[..head_end],
        tail_start - head_end,
        &text[tail_start..]
    )
}
