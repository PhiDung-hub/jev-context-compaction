use std::collections::BTreeMap;

use crate::model::{CallAction, CallAnswer, CallDecision, Message, ToolCall, ToolResult, ToolUse};

/// An item costing more than `1 / OVERSIZED_SHARE` of the keep budget ranks after all others.
const OVERSIZED_SHARE: usize = 4;

/// Keep the calls and full results Jev ranks highest until `budget_ratio` of the
/// unpinned calls' characters is spent. A full result ranks by
/// `keep_call * keep_result` and also pays for its call; pinned calls are free.
/// Items over a quarter of the budget rank last, spending only what the rest left.
#[must_use]
pub fn decide_calls(
    calls: &[ToolCall],
    answers: &BTreeMap<String, CallAnswer>,
    budget_ratio: f64,
    head_chars: usize,
) -> Vec<CallDecision> {
    let answer = |call: &ToolCall| {
        answers.get(&call.id).copied().unwrap_or(CallAnswer {
            keep_call: 1.0,
            keep_result: 1.0,
        })
    };
    let costs: Vec<_> = calls
        .iter()
        .map(|call| call_costs(call, head_chars))
        .collect();
    let candidates = || (0..calls.len()).filter(|index| !calls[*index].pinned);
    let total = candidates()
        .map(|index| costs[index].0 + costs[index].1)
        .sum();
    let mut budget = budget_chars(total, budget_ratio);
    let oversized = |index: usize, full: bool| {
        costs[index].0 + if full { costs[index].1 } else { 0 } > budget / OVERSIZED_SHARE
    };
    let mut ranked: Vec<_> = candidates()
        .flat_map(|index| {
            let answer = answer(&calls[index]);
            [
                (oversized(index, false), answer.keep_call, index, false),
                (
                    oversized(index, true),
                    answer.keep_call * answer.keep_result,
                    index,
                    true,
                ),
            ]
        })
        .collect();
    // Oversized items last, so one cannot crowd out the rest; then highest probability
    // first, and ties go to the newer call.
    ranked.sort_by(|left, right| {
        (left.0.cmp(&right.0))
            .then(right.1.total_cmp(&left.1))
            .then(right.2.cmp(&left.2))
    });
    let mut actions: Vec<_> = calls
        .iter()
        .map(|call| {
            if call.pinned {
                CallAction::Keep
            } else {
                CallAction::DropCall
            }
        })
        .collect();
    for (_, _, index, full) in ranked {
        let cost = match (actions[index], full) {
            (CallAction::DropCall, false) => costs[index].0,
            (CallAction::DropCall, true) => costs[index].0 + costs[index].1,
            (CallAction::DropResult, true) => costs[index].1,
            _ => continue,
        };
        if cost <= budget {
            budget -= cost;
            actions[index] = if full {
                CallAction::Keep
            } else {
                CallAction::DropResult
            };
        }
    }
    calls
        .iter()
        .zip(actions)
        .map(|(call, action)| {
            let reason = match (call.pinned, action) {
                (true, _) => "pinned",
                (_, CallAction::Keep) => "kept",
                (_, CallAction::DropResult) => "result_dropped",
                (_, CallAction::DropCall) => "call_dropped",
            };
            let answer = answer(call);
            CallDecision {
                id: call.id.clone(),
                tool: call.tool.clone(),
                keep_call: answer.keep_call,
                keep_result: answer.keep_result,
                action,
                reason,
            }
        })
        .collect()
}

/// Characters a call keeps with its result truncated, and the extra its full result adds.
fn call_costs(call: &ToolCall, head_chars: usize) -> (usize, usize) {
    let input = serde_json::to_vec(&call.input).map_or(20, |encoded| encoded.len());
    let truncated = call.result_chars.min(head_chars.saturating_add(120));
    (input + truncated, call.result_chars - truncated)
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
fn budget_chars(total: usize, ratio: f64) -> usize {
    (total as f64 * ratio) as usize
}

#[must_use]
pub fn apply_decisions(
    messages: &[Message],
    decisions: &[CallDecision],
    calls: &[ToolCall],
    head_chars: usize,
) -> Vec<Message> {
    let by_id: BTreeMap<_, _> = calls.iter().map(|call| (&call.id, call)).collect();
    let actions: BTreeMap<_, _> = decisions
        .iter()
        .filter(|decision| decision.action != CallAction::Keep)
        .filter_map(|decision| {
            by_id
                .get(&decision.id)
                .map(|call| (call.tool_use_id.as_str(), decision.action))
        })
        .collect();
    messages
        .iter()
        .filter_map(|message| rebuild_message(message, &actions, head_chars))
        .collect()
}

fn rebuild_message(
    message: &Message,
    actions: &BTreeMap<&str, CallAction>,
    head_chars: usize,
) -> Option<Message> {
    let touched = message
        .tool_uses
        .iter()
        .any(|tool| actions.contains_key(tool.tool_use_id.as_str()))
        || message
            .tool_results
            .iter()
            .any(|result| actions.contains_key(result.tool_use_id.as_str()));
    if !touched {
        return Some(message.clone());
    }
    let tool_uses: Vec<ToolUse> = message
        .tool_uses
        .iter()
        .filter(|tool| actions.get(tool.tool_use_id.as_str()) != Some(&CallAction::DropCall))
        .map(|tool| {
            let mut tool = tool.clone();
            if actions.get(tool.tool_use_id.as_str()) == Some(&CallAction::DropResult) {
                if let Some(text) = &tool.text {
                    tool.text = Some(truncated_result(text, tool.is_error, head_chars));
                }
            }
            tool
        })
        .collect();
    let tool_results: Vec<ToolResult> = message
        .tool_results
        .iter()
        .filter(|result| actions.get(result.tool_use_id.as_str()) != Some(&CallAction::DropCall))
        .map(|result| {
            let mut result = result.clone();
            if actions.get(result.tool_use_id.as_str()) == Some(&CallAction::DropResult) {
                result.text = truncated_result(&result.text, result.is_error, head_chars);
            }
            result
        })
        .collect();
    if message.text.trim().is_empty() && tool_uses.is_empty() && tool_results.is_empty() {
        return None;
    }
    if tool_uses == message.tool_uses && tool_results == message.tool_results {
        return Some(message.clone());
    }
    Some(Message {
        role: message.role,
        text: message.text.clone(),
        tool_uses,
        tool_results,
        handle: None,
        extra: message.extra.clone(),
    })
}

fn truncated_result(text: &str, is_error: bool, head_chars: usize) -> String {
    if text.len() <= head_chars.saturating_add(120) {
        return text.to_owned();
    }
    let mut end = head_chars.min(text.len());
    while !text.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    let head = if end == 0 {
        String::new()
    } else {
        format!("{}\n", &text[..end])
    };
    format!(
        "{head}[jev-context-compaction truncated {} chars of this tool result{}; re-run the tool if needed]",
        text.len().saturating_sub(end),
        if is_error { " (error)" } else { "" }
    )
}
