use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::time::Instant;

use typesafe_ai::Json;

use super::super::representation::{
    abridge, compact_old_calls, goal_from_messages, history_entries, input_stage, is_pinned,
    keep_newest_heads, merge_call_runs, newest_heads_within, shrink_order, state, state_tokens,
};
use super::{HEAD_SHARE_PERCENT, INPUT_LIMITS, TEXT_HEAD, TEXT_TAIL, fit_state, with_heads};
use crate::model::{
    CompactOptions, CompactionError, Message, Result, Role, ToolCall, ToolResult, ToolUse,
};
use crate::state::{CompactionState, FittedState, HistoryCall, HistoryEntry, collect_tool_calls};
use crate::tokens::estimate_tokens;

#[derive(serde::Deserialize)]
struct Payload {
    messages: Vec<Message>,
}

fn within(state: CompactionState, limit: usize, stage_label: &str) -> Result<Option<FittedState>> {
    let tokens = state_tokens(&state)?;
    let heads = heads(&state.history).count() + compacted_heads(&state.history);
    Ok((tokens <= limit).then(|| FittedState {
        state,
        tokens,
        stage: with_heads(stage_label, heads),
    }))
}

fn compacted_heads(history: &[HistoryEntry]) -> usize {
    history
        .iter()
        .flat_map(|entry| &entry.tool_calls)
        .filter(|call| matches!(call, HistoryCall::Compact(line) if line.contains("; head: ")))
        .count()
}

/// Like [`within`], but first drops result heads, oldest first and never the newest `keep`,
/// until the state fits.
fn within_dropping_heads(
    goal: &str,
    mut history: Vec<HistoryEntry>,
    limit: usize,
    keep: usize,
    stage_label: &str,
) -> Result<Option<FittedState>> {
    loop {
        let state = state(goal.to_owned(), history);
        let tokens = state_tokens(&state)?;
        if tokens <= limit {
            let kept = heads(&state.history).count();
            return Ok(Some(FittedState {
                state,
                tokens,
                stage: if kept == 0 {
                    stage_label.to_owned()
                } else {
                    format!("{stage_label}, {kept} heads")
                },
            }));
        }
        history = state.history;
        if !drop_heads(&mut history, tokens - limit, keep) {
            return Ok(None);
        }
    }
}

fn heads(history: &[HistoryEntry]) -> impl Iterator<Item = &String> {
    history
        .iter()
        .flat_map(|entry| &entry.tool_calls)
        .filter_map(|call| match call {
            HistoryCall::Structured { head, .. } => head.as_ref(),
            HistoryCall::Compact(_) => None,
        })
}

/// Drop heads oldest first, never the newest `keep`, until about `excess` tokens are freed;
/// false when none were left.
fn drop_heads(history: &mut [HistoryEntry], excess: usize, keep: usize) -> bool {
    let mut freed = 0;
    let droppable = heads(history).count().saturating_sub(keep);
    let slots = history
        .iter_mut()
        .flat_map(|entry| &mut entry.tool_calls)
        .filter_map(|call| match call {
            HistoryCall::Structured { head, .. } => head.as_mut().is_some().then_some(head),
            HistoryCall::Compact(_) => None,
        })
        .take(droppable);
    for head in slots {
        if freed >= excess {
            break;
        }
        let encoded = serde_json::to_string(&head.take()).unwrap_or_default();
        freed += estimate_tokens(&format!(",\"head\":{encoded}"));
    }
    freed > 0
}

/// The pre-incremental fitter with a head share: re-serialises the whole state after every step.
fn reference_fit(
    messages: &[Message],
    calls: &[ToolCall],
    options: &CompactOptions,
) -> Result<FittedState> {
    let share = options.max_state_tokens * HEAD_SHARE_PERCENT / 100;
    reference_fit_with_share(messages, calls, options, share)
        .or_else(|_| reference_fit_with_share(messages, calls, options, 0))
}

fn reference_fit_with_share(
    messages: &[Message],
    calls: &[ToolCall],
    options: &CompactOptions,
    share: usize,
) -> Result<FittedState> {
    let goal = goal_from_messages(messages);
    let limit = options.max_state_tokens;
    let head_chars = options.truncate_head_chars;
    let keep = newest_heads_within(&history_entries(messages, calls, 0, head_chars), share)?;
    for input in INPUT_LIMITS {
        let history = history_entries(messages, calls, input, head_chars);
        let stage = input_stage(input);
        if let Some(fitted) = within_dropping_heads(&goal, history, limit, keep, stage)? {
            return Ok(fitted);
        }
    }
    let (total, recent) = (messages.len(), options.preserve_recent_messages);
    let mut history = history_entries(messages, calls, INPUT_LIMITS[2], head_chars);
    keep_newest_heads(&mut history, keep);
    let order = shrink_order(&history, total, recent);
    for index in &order {
        let text = &history[*index].text;
        if text.len() > TEXT_HEAD + TEXT_TAIL + 40 {
            history[*index].text = abridge(text, TEXT_HEAD, TEXT_TAIL);
            let fitted = within(
                state(goal.clone(), history.clone()),
                limit,
                "texts abridged",
            )?;
            if let Some(fitted) = fitted {
                return Ok(fitted);
            }
        }
    }
    for index in &order {
        let entry = &mut history[*index];
        if !is_pinned(entry.i, total, recent) && !entry.text.is_empty() {
            entry.text = format!("[… {} chars omitted …]", messages[entry.i].text.len());
            let stage = "old messages collapsed";
            if let Some(fitted) = within(state(goal.clone(), history.clone()), limit, stage)? {
                return Ok(fitted);
            }
        }
    }
    compact_old_calls(&mut history, calls, total, recent);
    let fitted = within(
        state(goal.clone(), history.clone()),
        limit,
        "old calls compacted",
    )?;
    if let Some(fitted) = fitted {
        return Ok(fitted);
    }
    history.retain(|entry| is_pinned(entry.i, total, recent) || !entry.tool_calls.is_empty());
    merge_call_runs(&mut history, total, recent);
    within(state(goal, history), limit, "old calls merged")?
        .ok_or_else(|| CompactionError::StateTooLarge(String::new()))
}

/// Deterministic transcript of `turns` assistant/tool rounds with varied sizes.
fn synthetic(turns: usize) -> Vec<Message> {
    let mut seed = 0x2545_f491_u64;
    let mut next = move |modulo: usize| {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        usize::try_from(seed >> 33).unwrap_or(0) % modulo
    };
    let words = |count: usize, salt: usize| {
        (0..count).fold(String::new(), |mut text, word| {
            let _ = writeln!(text, "w{}rd_{} \"q\"", word * 7 + salt, salt % 97);
            text
        })
    };
    let message = |role, text: String| Message {
        role,
        text,
        tool_uses: vec![],
        tool_results: vec![],
        handle: None,
        extra: BTreeMap::new(),
    };
    let mut messages = vec![message(Role::User, words(40, 1))];
    for turn in 0..turns {
        let id = format!("toolu_{turn}");
        let said = if turn % 3 == 0 { next(4) * next(60) } else { 0 };
        let mut assistant = message(Role::Assistant, words(said, turn));
        assistant.tool_uses.push(ToolUse {
            tool_use_id: id.clone(),
            tool: ["Read", "Bash", "Grep"][turn % 3].to_owned(),
            input: BTreeMap::from([("command".to_owned(), Json::from(words(next(40), turn)))]),
            text: None,
            is_error: false,
            extra: BTreeMap::new(),
        });
        let mut result = message(Role::User, String::new());
        result.tool_results.push(ToolResult {
            tool_use_id: id,
            text: words(next(400), turn),
            is_error: turn % 11 == 0,
            extra: BTreeMap::new(),
        });
        messages.extend([assistant, result]);
        if turn % 9 == 0 {
            messages.push(message(Role::User, words(next(300), turn)));
        }
    }
    messages
}

fn outcome(fitted: Result<FittedState>) -> Result<(String, usize, String)> {
    let fitted = fitted?;
    assert_eq!(fitted.tokens, state_tokens(&fitted.state)?, "token drift");
    Ok((
        fitted.stage,
        fitted.tokens,
        serde_json::to_string(&fitted.state)?,
    ))
}

/// `inputs<=200, 7 heads` -> `inputs<=200, heads`.
fn stage_kind(stage: &str) -> String {
    stage
        .split_once(", ")
        .map_or_else(|| stage.to_owned(), |(stage, _)| format!("{stage}, heads"))
}

/// Stages the equivalence sweep reaches without heads and with 300-char heads. With heads,
/// every stage keeps the newest heads, and `old calls merged` without them is the fallback
/// once even the head share cannot fit.
const STAGE_KINDS: [&str; 18] = [
    "0: full",
    "0: inputs<=200",
    "0: inputs<=60",
    "0: texts abridged",
    "0: old messages collapsed",
    "0: old calls compacted",
    "0: old calls merged",
    "0: too large",
    "300: full, heads",
    "300: inputs<=200, heads",
    "300: inputs<=60, heads",
    "300: texts abridged, heads",
    "300: old messages collapsed, heads",
    "300: old calls compacted, heads",
    "300: old calls merged, heads",
    "300: old calls compacted",
    "300: old calls merged",
    "300: too large",
];

#[test]
fn incremental_fit_matches_full_reserialisation() -> Result<()> {
    let messages = synthetic(100);
    let calls = collect_tool_calls(&messages, 6);
    let goal = goal_from_messages(&messages);
    let head_chars = CompactOptions::default().truncate_head_chars;
    let full = state_tokens(&state(
        goal,
        history_entries(&messages, &calls, INPUT_LIMITS[0], head_chars),
    ))?;
    let mut stages = BTreeSet::new();
    for head_chars in [0, head_chars] {
        // Every budget below ~10k is too large; 12k is where the fallback compacts old calls.
        let budgets = std::iter::successors(Some(9_000_usize), |budget| Some(budget + budget / 8));
        for budget in budgets
            .take_while(|budget| *budget < full)
            .chain([12_000, full])
        {
            let options = CompactOptions {
                max_state_tokens: budget,
                truncate_head_chars: head_chars,
                ..CompactOptions::default()
            };
            let expected = outcome(reference_fit(&messages, &calls, &options));
            let actual = outcome(fit_state(&messages, &calls, &options));
            let stage = match (expected, actual) {
                (Ok(expected), Ok(actual)) => {
                    assert_eq!(expected, actual, "budget {budget}, heads {head_chars}");
                    stage_kind(&expected.0)
                }
                (Err(_), Err(_)) => "too large".to_owned(),
                (expected, actual) => panic!("budget {budget}: {expected:?} vs {actual:?}"),
            };
            stages.insert(format!("{head_chars}: {stage}"));
        }
    }
    let expected = STAGE_KINDS
        .map(str::to_owned)
        .into_iter()
        .collect::<BTreeSet<_>>();
    assert_eq!(stages, expected, "every fitting stage exercised");
    Ok(())
}

/// `cargo test -p jev-context-compaction --release --lib fit_timing -- --ignored --nocapture`
#[test]
#[ignore = "timing report; run in release"]
fn fit_timing() -> Result<()> {
    let messages = match std::env::var("JEV_FIT_PAYLOAD") {
        Ok(path) => serde_json::from_slice::<Payload>(&std::fs::read(path)?)?.messages,
        Err(_) => synthetic(250),
    };
    let calls = collect_tool_calls(&messages, 6);
    let options = CompactOptions::default();
    let chars: usize = messages
        .iter()
        .map(|message| serde_json::to_string(message).map_or(0, |json| json.len()))
        .sum();
    assert_eq!(
        outcome(reference_fit(&messages, &calls, &options))?,
        outcome(fit_state(&messages, &calls, &options))?
    );
    for (name, reference) in [("reference", true), ("incremental", false)] {
        let mut times = Vec::new();
        let mut stage = String::new();
        for _ in 0..7 {
            let started = Instant::now();
            let fitted = if reference {
                reference_fit(&messages, &calls, &options)?
            } else {
                fit_state(&messages, &calls, &options)?
            };
            times.push(started.elapsed().as_secs_f64() * 1_000.0);
            stage = fitted.stage;
        }
        times.sort_by(f64::total_cmp);
        println!(
            "{name}: p50 {:.1} ms (min {:.1}, max {:.1}); {} messages, {chars} chars, {stage}",
            times[3],
            times[0],
            times[6],
            messages.len()
        );
    }
    Ok(())
}
