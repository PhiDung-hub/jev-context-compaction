//! Verbatim context compaction guided by `TypeSafe` Jev judgments.

mod compact;
mod decision;
mod model;
mod questions;
mod state;
mod tokens;

pub use compact::{compact, reduction_ratio};
pub use decision::{apply_decisions, decide_call};
pub use model::{
    CallAction, CallAnswer, CallDecision, CompactOptions, CompactResult, CompactStats,
    CompactionError, Message, Role, ToolCall, ToolResult, ToolUse,
};
pub use state::{CompactionState, FittedState, collect_tool_calls, fit_state};
pub use tokens::estimate_tokens;
