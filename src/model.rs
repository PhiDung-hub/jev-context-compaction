use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use typesafe_ai::Json;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ToolUse {
    pub tool_use_id: String,
    pub tool: String,
    #[serde(default)]
    pub input: BTreeMap<String, Json>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, rename = "isError", skip_serializing_if = "is_false")]
    pub is_error: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ToolResult {
    pub tool_use_id: String,
    pub text: String,
    #[serde(default, rename = "isError", skip_serializing_if = "is_false")]
    pub is_error: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub role: Role,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub tool_uses: Vec<ToolUse>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_results: Vec<ToolResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ToolCall {
    pub id: String,
    pub tool_use_id: String,
    pub tool: String,
    pub input: BTreeMap<String, Json>,
    pub call_index: usize,
    pub result_chars: usize,
    pub is_error: bool,
    pub pinned: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CallAnswer {
    pub keep_call: f64,
    pub keep_result: f64,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CallAction {
    Keep,
    DropResult,
    DropCall,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallDecision {
    pub id: String,
    pub tool: String,
    pub keep_call: f64,
    pub keep_result: f64,
    pub action: CallAction,
    pub reason: &'static str,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CompactOptions {
    pub goal: String,
    pub model: Option<String>,
    pub keep_threshold: f64,
    pub preserve_recent_messages: usize,
    pub max_state_tokens: usize,
    pub max_combined_tokens: usize,
    pub max_state_question_tokens: usize,
    pub max_parallel_requests: usize,
    pub request_timeout_ms: u64,
    pub truncate_head_chars: usize,
}

impl Default for CompactOptions {
    fn default() -> Self {
        Self {
            goal: String::new(),
            model: None,
            keep_threshold: 0.5,
            preserve_recent_messages: 6,
            max_state_tokens: 25_000,
            max_combined_tokens: 60_000,
            max_state_question_tokens: 30_000,
            max_parallel_requests: 4,
            request_timeout_ms: 3_000,
            truncate_head_chars: 300,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactStats {
    pub messages_before: usize,
    pub messages_after: usize,
    pub chars_before: usize,
    pub chars_after: usize,
    pub calls: usize,
    pub kept: usize,
    pub results_dropped: usize,
    pub calls_dropped: usize,
    pub pinned: usize,
    pub state_tokens: usize,
    pub state_stage: String,
    pub requests: usize,
    pub input_tokens: u64,
    pub elapsed_ms: u128,
}

#[derive(Clone, Debug, Serialize)]
pub struct CompactResult {
    pub messages: Vec<Message>,
    pub decisions: Vec<CallDecision>,
    pub stats: CompactStats,
}

#[derive(Debug, thiserror::Error)]
pub enum CompactionError {
    #[error("invalid compaction option: {0}")]
    InvalidOption(String),
    #[error("history cannot fit Jev state budget: {0}")]
    StateTooLarge(String),
    #[error("a question cannot fit the request budget")]
    QuestionTooLarge,
    #[error("Jev response is missing Noul answer {0}")]
    MissingAnswer(String),
    #[error(transparent)]
    TypeSafe(#[from] typesafe_ai::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[allow(clippy::trivially_copy_pass_by_ref)]
pub(crate) fn is_false(value: &bool) -> bool {
    !value
}

pub(crate) type Result<T> = std::result::Result<T, CompactionError>;
