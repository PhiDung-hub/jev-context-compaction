use std::collections::BTreeMap;

use typesafe_ai::{Question, Questions};

use crate::model::{CallAnswer, CompactOptions, CompactionError, Result, ToolCall};
use crate::tokens::estimate_tokens;

const REQUEST_OVERHEAD_TOKENS: usize = 24;

pub(crate) fn questions_for(call: &ToolCall) -> Questions {
    BTreeMap::from([
        (
            format!("call_{}", call.id),
            Question::noul(format!(
                "Does tool call {} ({}) still matter for the assistant's next work? Judge its input in `history`.",
                call.id, call.tool
            )),
        ),
        (
            format!("result_{}", call.id),
            Question::noul(format!(
                "Will the assistant need to re-read the exact full output of tool call {} ({}) because it is a file still being edited or output still being fixed?",
                call.id, call.tool
            )),
        ),
    ])
}

pub(crate) fn batch_calls(
    calls: &[ToolCall],
    state_tokens: usize,
    options: &CompactOptions,
) -> Result<Vec<Vec<ToolCall>>> {
    let available = options
        .max_combined_tokens
        .saturating_sub(state_tokens)
        .saturating_sub(REQUEST_OVERHEAD_TOKENS);
    let mut batches = Vec::new();
    let mut current = Vec::new();
    let mut current_tokens = 0_usize;
    for call in calls {
        let questions = questions_for(call);
        let pair_tokens = estimate_tokens(&serde_json::to_string(&questions)?);
        let longest = questions
            .values()
            .map(|question| estimate_tokens(&serde_json::to_string(question).unwrap_or_default()))
            .max()
            .unwrap_or(0);
        if pair_tokens > available
            || state_tokens.saturating_add(longest) > options.max_state_question_tokens
        {
            return Err(CompactionError::QuestionTooLarge);
        }
        if !current.is_empty() && current_tokens.saturating_add(pair_tokens) > available {
            batches.push(current);
            current = Vec::new();
            current_tokens = 0;
        }
        current.push(call.clone());
        current_tokens = current_tokens.saturating_add(pair_tokens);
    }
    if !current.is_empty() {
        batches.push(current);
    }
    Ok(batches)
}

pub(crate) fn answer_for(
    response: &typesafe_ai::SystemOneResponse,
    call: &ToolCall,
) -> Result<CallAnswer> {
    let call_name = format!("call_{}", call.id);
    let result_name = format!("result_{}", call.id);
    let keep_call = response
        .noul(&call_name)
        .ok_or_else(|| CompactionError::MissingAnswer(call_name))?
        .noul;
    let keep_result = response
        .noul(&result_name)
        .ok_or_else(|| CompactionError::MissingAnswer(result_name))?
        .noul;
    Ok(CallAnswer {
        keep_call,
        keep_result,
    })
}
