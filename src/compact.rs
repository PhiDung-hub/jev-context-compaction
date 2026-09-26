use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::time::{Duration, Instant};

use typesafe_ai::{Client, RequestOptions, RetryPolicy};
use typesafe_ai_common::{SystemOneCall, system_one_batch};

use crate::decision::{apply_decisions, decide_calls};
use crate::model::{
    CallAnswer, CompactOptions, CompactResult, CompactStats, CompactionError, Message, Result,
};
use crate::questions::{answer_for, batch_calls, questions_for};
use crate::state::{collect_tool_calls, fit_state};

/// Compact a transcript using the fewest Jev fan-out requests allowed by the budgets.
///
/// # Errors
///
/// Returns configuration, fitting, transport, API, or response errors. Callers
/// should fall back to their ordinary summarizer on error.
pub async fn compact(
    client: &Client,
    messages: &[Message],
    options: &CompactOptions,
) -> Result<CompactResult> {
    validate_options(options)?;
    let started = Instant::now();
    let calls = collect_tool_calls(messages, options.preserve_recent_messages);
    let candidates: Vec<_> = calls.iter().filter(|call| !call.pinned).cloned().collect();
    let chars_before = message_chars_total(messages);
    let mut answers = BTreeMap::<String, CallAnswer>::new();
    let mut requests = 0;
    let mut input_tokens = 0_u64;
    let mut state_tokens = 0;
    let mut state_stage = String::new();

    if !candidates.is_empty() {
        let fitted = fit_state(messages, &calls, options)?;
        state_tokens = fitted.tokens;
        state_stage.clone_from(&fitted.stage);
        let batches = batch_calls(&candidates, fitted.tokens, options)?;
        requests = batches.len();
        let mut request_options = RequestOptions::default()
            .retry(RetryPolicy::disabled())
            .timeout(Duration::from_millis(options.request_timeout_ms));
        if let Some(model) = options.model.as_deref() {
            request_options = request_options.model(model);
        }
        let sdk_calls: Vec<_> = batches
            .iter()
            .map(|batch| {
                let questions: typesafe_ai::Questions = batch
                    .iter()
                    .flat_map(|call| questions_for(call).into_iter())
                    .collect();
                SystemOneCall::new(fitted.state.clone(), questions).options(request_options.clone())
            })
            .collect();
        let concurrency = NonZeroUsize::new(options.max_parallel_requests).ok_or_else(|| {
            CompactionError::InvalidOption("max_parallel_requests is zero".to_owned())
        })?;
        let responses = system_one_batch(client, sdk_calls, concurrency).await;
        for (batch, response) in batches.iter().zip(responses) {
            let response = response?;
            input_tokens = input_tokens.saturating_add(response.usage.input_tokens.unwrap_or(0));
            for call in batch {
                answers.insert(call.id.clone(), answer_for(&response, call)?);
            }
        }
    }

    let decisions = decide_calls(
        &calls,
        &answers,
        options.keep_budget_ratio,
        options.truncate_head_chars,
    );
    let compacted = apply_decisions(messages, &decisions, &calls, options.truncate_head_chars);
    let stats = CompactStats {
        messages_before: messages.len(),
        messages_after: compacted.len(),
        chars_before,
        chars_after: message_chars_total(&compacted),
        calls: calls.len(),
        kept: count(&decisions, "kept"),
        results_dropped: count(&decisions, "result_dropped"),
        calls_dropped: count(&decisions, "call_dropped"),
        pinned: count(&decisions, "pinned"),
        state_tokens,
        state_stage,
        requests,
        input_tokens,
        elapsed_ms: started.elapsed().as_millis(),
    };
    Ok(CompactResult {
        messages: compacted,
        decisions,
        stats,
    })
}

#[must_use]
pub fn reduction_ratio(result: &CompactResult) -> f64 {
    if result.stats.chars_before == 0 {
        return 0.0;
    }
    let removed = u32::try_from(
        result
            .stats
            .chars_before
            .saturating_sub(result.stats.chars_after),
    )
    .unwrap_or(u32::MAX);
    let before = u32::try_from(result.stats.chars_before).unwrap_or(u32::MAX);
    f64::from(removed) / f64::from(before)
}

fn validate_options(options: &CompactOptions) -> Result<()> {
    if !(0.0..=1.0).contains(&options.keep_budget_ratio) {
        return Err(CompactionError::InvalidOption(
            "keep_budget_ratio must be between zero and one".to_owned(),
        ));
    }
    if options.max_state_tokens == 0
        || options.max_combined_tokens == 0
        || options.max_state_question_tokens == 0
        || options.max_parallel_requests == 0
        || options.request_timeout_ms == 0
    {
        return Err(CompactionError::InvalidOption(
            "token budgets and request timeout must be positive".to_owned(),
        ));
    }
    Ok(())
}

fn count(decisions: &[crate::model::CallDecision], reason: &str) -> usize {
    decisions
        .iter()
        .filter(|decision| decision.reason == reason)
        .count()
}

fn message_chars_total(messages: &[Message]) -> usize {
    messages
        .iter()
        .map(|message| {
            message.text.len()
                + message
                    .tool_uses
                    .iter()
                    .map(|tool| {
                        serde_json::to_vec(&tool.input).map_or(20, |encoded| encoded.len())
                            + tool.text.as_ref().map_or(0, String::len)
                    })
                    .sum::<usize>()
                + message
                    .tool_results
                    .iter()
                    .map(|result| result.text.len())
                    .sum::<usize>()
        })
        .sum()
}
