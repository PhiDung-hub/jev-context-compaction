use std::collections::BTreeMap;

use httpmock::Method::POST;
use httpmock::MockServer;
use jev_context_compaction::{
    CallAction, CallAnswer, CallDecision, CompactOptions, FittedState, Message, Role, ToolResult,
    ToolUse, collect_tool_calls, compact, decide_calls, fit_state,
};
use typesafe_ai::{Client, Json};

fn text(role: Role, value: &str) -> Message {
    Message {
        role,
        text: value.to_owned(),
        tool_uses: vec![],
        tool_results: vec![],
        handle: None,
        extra: BTreeMap::new(),
    }
}

fn call(id: &str, tool: &str, path: &str) -> Message {
    Message {
        role: Role::Assistant,
        text: String::new(),
        tool_uses: vec![ToolUse {
            tool_use_id: id.to_owned(),
            tool: tool.to_owned(),
            input: BTreeMap::from([("file_path".to_owned(), Json::from(path))]),
            text: None,
            is_error: false,
            extra: BTreeMap::new(),
        }],
        tool_results: vec![],
        handle: Some(format!("call-{id}")),
        extra: BTreeMap::new(),
    }
}

fn result(id: &str, value: &str) -> Message {
    Message {
        role: Role::User,
        text: String::new(),
        tool_uses: vec![],
        tool_results: vec![ToolResult {
            tool_use_id: id.to_owned(),
            text: value.to_owned(),
            is_error: false,
            extra: BTreeMap::new(),
        }],
        handle: Some(format!("result-{id}")),
        extra: BTreeMap::new(),
    }
}

fn transcript() -> Vec<Message> {
    vec![
        text(
            Role::User,
            "Fix the failing test without editing generated files.",
        ),
        call("one", "Read", "src/old.rs"),
        result("one", &"old output\n".repeat(200)),
        call("two", "Bash", "cargo test"),
        result("two", "test failed in parser"),
        text(Role::Assistant, "I found the parser failure."),
    ]
}

fn fit(messages: &[Message], max_state_tokens: usize, head_chars: usize) -> FittedState {
    let options = CompactOptions {
        preserve_recent_messages: 0,
        max_state_tokens,
        truncate_head_chars: head_chars,
        ..CompactOptions::default()
    };
    fit_state(messages, &collect_tool_calls(messages, 0), &options).unwrap()
}

fn answers(probabilities: &[(f64, f64)]) -> BTreeMap<String, CallAnswer> {
    probabilities
        .iter()
        .enumerate()
        .map(|(index, &(keep_call, keep_result))| {
            let answer = CallAnswer {
                keep_call,
                keep_result,
            };
            (format!("t{}", index + 1), answer)
        })
        .collect()
}

fn many_calls(count: usize) -> Vec<Message> {
    let mut messages = vec![text(Role::User, "Fix the failing test.")];
    for index in 0..count {
        let id = format!("c{index}");
        messages.push(call(&id, "Read", &format!("src/{index}.rs")));
        messages.push(result(&id, &format!("result {index} {}", "x".repeat(990))));
    }
    messages.push(text(Role::Assistant, "Parser fix is next."));
    messages
}

#[test]
fn fitted_state_shows_result_heads_not_full_results() {
    let encoded = serde_json::to_string(&fit(&transcript(), 25_000, 300).state).unwrap();
    assert!(encoded.contains(r#""head":"old output\nold output"#));
    assert!(encoded.matches("old output").count() < 30);
}

#[test]
fn state_drops_oldest_heads_before_any_text() {
    let messages = many_calls(8);
    let with_heads = fit(&messages, 1_000_000, 300).tokens;
    let without = fit(&messages, 1_000_000, 0).tokens;
    let fitted = fit(&messages, usize::midpoint(with_heads, without), 300);
    let encoded = serde_json::to_string(&fitted.state).unwrap();

    assert!(fitted.stage.starts_with("full, "), "{}", fitted.stage);
    assert!(!encoded.contains("result 0 "));
    assert!(encoded.contains("result 7 "));
    assert!(encoded.contains("Parser fix is next."));
}

fn long_note_and_calls() -> Vec<Message> {
    let mut messages = many_calls(40);
    messages.insert(1, text(Role::Assistant, &"note ".repeat(2_000)));
    messages
}

#[test]
fn newest_heads_keep_a_fifth_of_the_budget_while_old_text_shrinks() {
    let messages = long_note_and_calls();
    for (budget, stage, oldest) in [
        (5_000, "texts abridged, 17 heads", 23),
        (3_000, "old calls compacted, 10 heads", 30),
    ] {
        let fitted = fit(&messages, budget, 300);
        let encoded = serde_json::to_string(&fitted.state).unwrap();

        assert_eq!(fitted.stage, stage);
        assert!(encoded.contains(&format!("result {oldest} ")), "{stage}");
        assert!(
            !encoded.contains(&format!("result {} ", oldest - 1)),
            "{stage}"
        );
        assert!(encoded.contains("result 39 "), "{stage}");
    }
}

#[test]
fn head_share_yields_when_nothing_else_fits() {
    assert_eq!(
        fit(&long_note_and_calls(), 1_200, 300).stage,
        "old calls merged"
    );
}

#[test]
fn oversized_items_spend_only_what_the_rest_leave() {
    let mut messages = many_calls(8);
    messages.insert(1, call("big", "Workflow", &"s".repeat(6_000)));
    messages.insert(2, result("big", "ok"));
    let calls = collect_tool_calls(&messages, 0);
    let mut probabilities = vec![(0.9, 0.9)];
    probabilities.extend([(0.5, 0.1); 8]);
    let actions: Vec<_> = decide_calls(&calls, &answers(&probabilities), 0.5, 300)
        .into_iter()
        .map(|decision| decision.action)
        .collect();

    assert_eq!(actions[0], CallAction::DropCall);
    assert!(
        actions[1..]
            .iter()
            .all(|action| *action != CallAction::DropCall)
    );
}

#[test]
fn budget_keeps_top_ranked_calls_even_below_half() {
    let messages = many_calls(3);
    let calls = collect_tool_calls(&messages, 0);
    let answers = answers(&[(0.2, 0.1), (0.4, 0.45), (0.3, 0.2)]);
    let actions: Vec<_> = decide_calls(&calls, &answers, 0.3, 300)
        .into_iter()
        .map(|decision| decision.action)
        .collect();
    assert_eq!(
        actions,
        [
            CallAction::DropCall,
            CallAction::DropResult,
            CallAction::DropResult
        ]
    );
}

#[test]
fn reserve_keeps_a_full_result_that_plain_calls_would_crowd_out() {
    let calls = collect_tool_calls(&many_calls(12), 0);
    let mut probabilities = vec![(0.6, 0.9)];
    probabilities.extend([(0.9, 0.1); 11]);
    let decisions = decide_calls(&calls, &answers(&probabilities), 0.4, 300);
    assert_eq!(decisions[0].action, CallAction::Keep);
}

#[test]
fn reserve_skips_full_results_below_the_floor() {
    let calls = collect_tool_calls(&many_calls(12), 0);
    let mut probabilities = vec![(0.6, 0.1)];
    probabilities.extend([(0.9, 0.05); 11]);
    let decisions = decide_calls(&calls, &answers(&probabilities), 0.4, 300);
    assert!(decisions.iter().all(|d| d.action != CallAction::Keep));
}

#[test]
fn unused_reserve_flows_back_to_calls() {
    // Every full result costs more than the reserve, so calls get the whole budget.
    let calls = collect_tool_calls(&many_calls(10), 0);
    let actions: Vec<_> = decide_calls(&calls, &answers(&[(0.9, 0.9); 10]), 0.3, 300)
        .into_iter()
        .map(|decision| decision.action)
        .collect();
    assert_eq!(
        actions
            .iter()
            .filter(|action| **action == CallAction::DropResult)
            .count(),
        6
    );
}

#[test]
fn kept_characters_stay_within_the_budget() {
    let messages = many_calls(10);
    let calls = collect_tool_calls(&messages, 0);
    let probabilities: Vec<_> = (1..=10).map(|n| (f64::from(n) / 25.0, 0.3)).collect();
    let answers = answers(&probabilities);
    let input = |index: usize| serde_json::to_vec(&calls[index].input).unwrap().len();
    let total: usize = (0..10)
        .map(|index| input(index) + calls[index].result_chars)
        .sum();
    for percent in [0_u16, 10, 25, 50, 100] {
        let kept: usize = decide_calls(&calls, &answers, f64::from(percent) / 100.0, 300)
            .iter()
            .enumerate()
            .map(|(index, decision)| match decision.action {
                CallAction::Keep => input(index) + calls[index].result_chars,
                CallAction::DropResult => input(index) + 420,
                CallAction::DropCall => 0,
            })
            .sum();
        let budget = total * usize::from(percent) / 100;
        assert!(kept <= budget, "{percent}%: kept {kept} of {budget}");
        assert!(
            kept + 1_100 > budget,
            "{percent}%: {kept} leaves {budget} unspent"
        );
    }
}

#[test]
fn unchanged_short_results_keep_the_host_handle() {
    let messages = transcript();
    let calls = collect_tool_calls(&messages, 0);
    let decisions = vec![CallDecision {
        id: calls[1].id.clone(),
        tool: calls[1].tool.clone(),
        keep_call: 0.9,
        keep_result: 0.1,
        action: CallAction::DropResult,
        reason: "result_dropped",
    }];
    let output = jev_context_compaction::apply_decisions(&messages, &decisions, &calls, 300);

    assert_eq!(output[4].handle.as_deref(), Some("result-two"));
    assert_eq!(output[4].tool_results[0].text, "test failed in parser");
}

#[tokio::test]
async fn compacts_all_candidates_in_one_native_fanout_request() {
    let server = MockServer::start_async().await;
    let request = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/v1/systemone")
                .body_includes("call_t1")
                .body_includes("result_t2")
                .body_includes(
                    "(Read) because it is a file still being edited or output still being fixed?",
                );
            then.status(200).json_body(serde_json::json!({
                "model": "jev-test",
                "usage": {"input_tokens": 700, "output_tokens": 80},
                "answers": {
                    "call_t1": {"type": "noul", "noul": 0.1},
                    "result_t1": {"type": "noul", "noul": 0.1},
                    "call_t2": {"type": "noul", "noul": 0.9},
                    "result_t2": {"type": "noul", "noul": 0.9}
                }
            }));
        })
        .await;
    let client = Client::builder()
        .api_key("test")
        .base_url(server.base_url())
        .build()
        .unwrap();
    let options = CompactOptions {
        preserve_recent_messages: 0,
        ..CompactOptions::default()
    };
    let output = compact(&client, &transcript(), &options).await.unwrap();

    request.assert_calls_async(1).await;
    assert_eq!(output.stats.requests, 1);
    assert_eq!(output.stats.input_tokens, 700);
    assert_eq!(
        output
            .decisions
            .iter()
            .map(|decision| decision.action)
            .collect::<Vec<_>>(),
        [CallAction::DropCall, CallAction::Keep]
    );
    assert!(!output.messages.iter().any(|message| {
        message
            .tool_uses
            .iter()
            .any(|tool| tool.tool_use_id == "one")
    }));
}

#[test]
fn unknown_host_fields_round_trip() {
    let row = serde_json::json!({
        "role": "user",
        "text": "",
        "toolUses": [{
            "tool_use_id": "one", "tool": "Agent", "input": {},
            "result": {"agentId": "a1", "content": [{"type": "text", "text": "done"}]},
            "agentId": "a1", "durationMs": 42
        }],
        "toolResults": [{
            "tool_use_id": "one", "text": "done", "isError": true,
            "result": {"stdout": "done", "interrupted": false}
        }],
        "handle": "h1",
        "hostTag": [1, 2.5, null]
    });
    let message: Message = serde_json::from_value(row.clone()).unwrap();
    assert_eq!(serde_json::to_value(&message).unwrap(), row);
}

#[test]
fn default_request_timeout_allows_large_context_fanout() {
    assert_eq!(CompactOptions::default().request_timeout_ms, 8_000);
}
