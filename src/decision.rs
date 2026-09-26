use std::collections::BTreeMap;

use crate::model::{CallAction, CallAnswer, CallDecision, Message, ToolCall, ToolResult, ToolUse};

#[must_use]
pub fn decide_call(call: &ToolCall, answer: CallAnswer, threshold: f64) -> CallDecision {
    let (action, reason) = if call.pinned {
        (CallAction::Keep, "pinned")
    } else if answer.keep_result >= threshold {
        (CallAction::Keep, "kept")
    } else if answer.keep_call >= threshold {
        (CallAction::DropResult, "result_dropped")
    } else {
        (CallAction::DropCall, "call_dropped")
    };
    CallDecision {
        id: call.id.clone(),
        tool: call.tool.clone(),
        keep_call: answer.keep_call,
        keep_result: answer.keep_result,
        action,
        reason,
    }
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
