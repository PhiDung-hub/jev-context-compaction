mod fit;
mod representation;

use std::collections::BTreeMap;

use serde::Serialize;

use crate::model::{Message, Role, ToolCall, ToolResult};

pub use fit::fit_state;

const STATE_CONTEXT: &str = "A coding assistant conversation is being compacted. History is oldest first. Tool outputs are represented by short result notes and, while space allows, their first characters (`head`). Decide what must remain for the assistant's next work; deleted tools can be run again.";

#[derive(Clone, Debug, Serialize)]
pub struct CompactionState {
    context: &'static str,
    goal: String,
    history: Vec<HistoryEntry>,
}

#[derive(Clone, Debug)]
pub struct FittedState {
    pub state: CompactionState,
    pub tokens: usize,
    pub stage: String,
}

#[derive(Clone, Debug, Serialize)]
struct HistoryEntry {
    i: usize,
    role: Role,
    text: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tool_calls: Vec<HistoryCall>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
enum HistoryCall {
    Structured {
        id: String,
        tool: String,
        input: String,
        result: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        head: Option<String>,
    },
    Compact(String),
}

/// Pair completed tool uses with their results and mark protected calls.
#[must_use]
pub fn collect_tool_calls(messages: &[Message], preserve_recent: usize) -> Vec<ToolCall> {
    let mut results = BTreeMap::<&str, (usize, &ToolResult)>::new();
    for (message_index, message) in messages.iter().enumerate() {
        for result in &message.tool_results {
            results.insert(&result.tool_use_id, (message_index, result));
        }
    }
    let mut calls = Vec::new();
    for (call_index, message) in messages.iter().enumerate() {
        for tool in &message.tool_uses {
            let Some((result_index, result)) = results.get(tool.tool_use_id.as_str()) else {
                continue;
            };
            calls.push(ToolCall {
                id: format!("t{}", calls.len() + 1),
                tool_use_id: tool.tool_use_id.clone(),
                tool: tool.tool.clone(),
                input: tool.input.clone(),
                call_index,
                result_chars: result.text.len(),
                is_error: result.is_error,
                pinned: representation::is_pinned(call_index, messages.len(), preserve_recent)
                    || representation::is_pinned(*result_index, messages.len(), preserve_recent),
            });
        }
    }
    calls
}
