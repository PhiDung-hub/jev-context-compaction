use std::collections::BTreeMap;

use jev_context_compaction::{CompactOptions, Message, Role, ToolResult, ToolUse, compact};
use typesafe_ai::{Client, Json};

#[tokio::test]
#[ignore = "requires TYPESAFE_API_KEY and makes a live API request"]
async fn live_compaction_uses_one_request() {
    let _ = dotenvy::dotenv();
    let tool_uses = (0..8)
        .map(|index| ToolUse {
            tool_use_id: format!("call-{index}"),
            tool: "Read".to_owned(),
            input: BTreeMap::from([(
                "file_path".to_owned(),
                Json::from(format!("src/module-{index}.rs")),
            )]),
            text: None,
            is_error: false,
        })
        .collect();
    let tool_results = (0..8)
        .map(|index| ToolResult {
            tool_use_id: format!("call-{index}"),
            text: format!("source content for module {index}"),
            is_error: false,
        })
        .collect();
    let messages = vec![
        Message {
            role: Role::User,
            text: "Fix the build and preserve the exact compiler error.".to_owned(),
            tool_uses: vec![],
            tool_results: vec![],
            handle: None,
        },
        Message {
            role: Role::Assistant,
            text: String::new(),
            tool_uses,
            tool_results: vec![],
            handle: None,
        },
        Message {
            role: Role::User,
            text: String::new(),
            tool_uses: vec![],
            tool_results,
            handle: None,
        },
    ];
    let options = CompactOptions {
        preserve_recent_messages: 0,
        ..CompactOptions::default()
    };
    let output = compact(&Client::from_env().unwrap(), &messages, &options)
        .await
        .unwrap();
    assert_eq!(output.stats.requests, 1);
    assert!(output.stats.input_tokens > 0);
    eprintln!(
        "live compaction: {} calls / {} questions, {} request, {} ms, {} input tokens",
        output.stats.calls,
        output.stats.calls * 2,
        output.stats.requests,
        output.stats.elapsed_ms,
        output.stats.input_tokens
    );
}
