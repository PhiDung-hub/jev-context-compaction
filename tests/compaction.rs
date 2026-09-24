use std::collections::BTreeMap;

use httpmock::Method::POST;
use httpmock::MockServer;
use jev_context_compaction::{
    CallAction, CompactOptions, Message, Role, ToolResult, ToolUse, collect_tool_calls, compact,
    fit_state,
};
use typesafe_ai::{Client, Json};

fn text(role: Role, value: &str) -> Message {
    Message {
        role,
        text: value.to_owned(),
        tool_uses: vec![],
        tool_results: vec![],
        handle: None,
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
        }],
        tool_results: vec![],
        handle: Some(format!("call-{id}")),
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
        }],
        handle: Some(format!("result-{id}")),
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

#[test]
fn fitted_state_omits_verbatim_tool_results() {
    let messages = transcript();
    let options = CompactOptions {
        preserve_recent_messages: 0,
        ..CompactOptions::default()
    };
    let calls = collect_tool_calls(&messages, 0);
    let fitted = fit_state(&messages, &calls, &options).unwrap();
    let encoded = serde_json::to_string(&fitted.state).unwrap();
    assert!(!encoded.contains("old output"));
    assert!(encoded.contains("chars omitted"));
}

#[test]
fn unchanged_short_results_keep_the_host_handle() {
    let messages = transcript();
    let calls = collect_tool_calls(&messages, 0);
    let decisions = vec![jev_context_compaction::decide_call(
        &calls[1],
        jev_context_compaction::CallAnswer {
            keep_call: 0.9,
            keep_result: 0.1,
        },
        0.5,
    )];
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
                .body_includes("result_t2");
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
