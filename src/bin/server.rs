//! Local HTTP bridge for sandboxed plugin hosts.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    routing::{get, post},
};
use jev_context_compaction::{CompactOptions, CompactResult, CompactionError, Message, compact};
use serde::{Deserialize, Serialize};
use typesafe_ai::Client;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CompactRequest {
    messages: Vec<Message>,
    #[serde(default)]
    options: CompactOptions,
}

#[derive(Clone)]
struct AppState {
    client: Client,
    usage: Arc<UsageCircuit>,
    metrics: Arc<Metrics>,
}

#[derive(Default)]
struct Metrics {
    attempts: AtomicU64,
    succeeded: AtomicU64,
    failed: AtomicU64,
    jev_requests: AtomicU64,
    jev_input_tokens: AtomicU64,
    calls_seen: AtomicU64,
    calls_dropped: AtomicU64,
    results_dropped: AtomicU64,
    chars_before: AtomicU64,
    chars_after: AtomicU64,
    elapsed_ms: AtomicU64,
    last_elapsed_ms: AtomicU64,
}

impl Metrics {
    fn success(&self, result: &CompactResult) {
        let stats = &result.stats;
        self.succeeded.fetch_add(1, Ordering::Relaxed);
        self.jev_requests
            .fetch_add(as_u64(stats.requests), Ordering::Relaxed);
        self.jev_input_tokens
            .fetch_add(stats.input_tokens, Ordering::Relaxed);
        self.calls_seen
            .fetch_add(as_u64(stats.calls), Ordering::Relaxed);
        self.calls_dropped
            .fetch_add(as_u64(stats.calls_dropped), Ordering::Relaxed);
        self.results_dropped
            .fetch_add(as_u64(stats.results_dropped), Ordering::Relaxed);
        self.chars_before
            .fetch_add(as_u64(stats.chars_before), Ordering::Relaxed);
        self.chars_after
            .fetch_add(as_u64(stats.chars_after), Ordering::Relaxed);
        let elapsed = as_u64(stats.elapsed_ms);
        self.elapsed_ms.fetch_add(elapsed, Ordering::Relaxed);
        self.last_elapsed_ms.store(elapsed, Ordering::Relaxed);
    }

    fn snapshot(&self) -> Usage {
        Usage {
            attempts: self.attempts.load(Ordering::Relaxed),
            succeeded: self.succeeded.load(Ordering::Relaxed),
            failed: self.failed.load(Ordering::Relaxed),
            jev_requests: self.jev_requests.load(Ordering::Relaxed),
            jev_input_tokens: self.jev_input_tokens.load(Ordering::Relaxed),
            calls_seen: self.calls_seen.load(Ordering::Relaxed),
            calls_dropped: self.calls_dropped.load(Ordering::Relaxed),
            results_dropped: self.results_dropped.load(Ordering::Relaxed),
            chars_before: self.chars_before.load(Ordering::Relaxed),
            chars_after: self.chars_after.load(Ordering::Relaxed),
            elapsed_ms: self.elapsed_ms.load(Ordering::Relaxed),
            last_elapsed_ms: self.last_elapsed_ms.load(Ordering::Relaxed),
        }
    }
}

fn as_u64(value: impl TryInto<u64>) -> u64 {
    value.try_into().unwrap_or(u64::MAX)
}

struct UsageCircuit {
    paused: AtomicBool,
    pause_file: Option<PathBuf>,
}

impl UsageCircuit {
    fn new(pause_file: Option<PathBuf>) -> Self {
        let paused = pause_file.as_deref().is_some_and(Path::exists);
        Self {
            paused: AtomicBool::new(paused),
            pause_file,
        }
    }

    fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
    }

    fn pause(&self) -> std::io::Result<()> {
        self.paused.store(true, Ordering::Release);
        let Some(path) = &self.pause_file else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, b"usage_exhausted\n")
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BridgeError {
    code: &'static str,
    disable_hooks: bool,
    message: String,
}

#[derive(Serialize)]
struct Health {
    status: &'static str,
    usage: Usage,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Usage {
    attempts: u64,
    succeeded: u64,
    failed: u64,
    jev_requests: u64,
    jev_input_tokens: u64,
    calls_seen: u64,
    calls_dropped: u64,
    results_dropped: u64,
    chars_before: u64,
    chars_after: u64,
    elapsed_ms: u64,
    last_elapsed_ms: u64,
}

#[derive(Debug, thiserror::Error)]
enum ServerError {
    #[error(transparent)]
    Address(#[from] std::net::AddrParseError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    TypeSafe(#[from] typesafe_ai::Error),
}

#[tokio::main]
async fn main() -> Result<(), ServerError> {
    let address = std::env::var("FAST_JEV_LISTEN")
        .unwrap_or_else(|_| "127.0.0.1:8787".to_owned())
        .parse::<SocketAddr>()?;
    let pause_file = std::env::var_os("FAST_JEV_PAUSE_FILE").map(PathBuf::from);
    let listener = tokio::net::TcpListener::bind(address).await?;
    let app = app(Client::from_env()?, pause_file);
    eprintln!("jev-context-compaction listening on http://{address}");
    axum::serve(listener, app).await?;
    Ok(())
}

fn app(client: Client, pause_file: Option<PathBuf>) -> Router {
    Router::new()
        .route("/compact", post(compact_route))
        .route("/health", get(health_route))
        .with_state(AppState {
            client,
            usage: Arc::new(UsageCircuit::new(pause_file)),
            metrics: Arc::new(Metrics::default()),
        })
}

async fn compact_route(
    State(state): State<AppState>,
    Json(request): Json<CompactRequest>,
) -> Result<Json<CompactResult>, (StatusCode, Json<BridgeError>)> {
    state.metrics.attempts.fetch_add(1, Ordering::Relaxed);
    if state.usage.is_paused() {
        state.metrics.failed.fetch_add(1, Ordering::Relaxed);
        return Err(usage_exhausted("TypeSafe usage circuit is paused"));
    }
    match compact(&state.client, &request.messages, &request.options).await {
        Ok(result) => {
            state.metrics.success(&result);
            Ok(Json(result))
        }
        Err(error) if is_usage_exhausted(&error) => {
            state.metrics.failed.fetch_add(1, Ordering::Relaxed);
            if let Err(write_error) = state.usage.pause() {
                eprintln!("failed to persist usage circuit: {write_error}");
            }
            Err(usage_exhausted(&error.to_string()))
        }
        Err(error) => {
            state.metrics.failed.fetch_add(1, Ordering::Relaxed);
            Err((
                StatusCode::BAD_GATEWAY,
                Json(BridgeError {
                    code: "upstream_error",
                    disable_hooks: false,
                    message: error.to_string(),
                }),
            ))
        }
    }
}

async fn health_route(State(state): State<AppState>) -> Json<Health> {
    Json(Health {
        status: if state.usage.is_paused() {
            "usage_exhausted"
        } else {
            "ready"
        },
        usage: state.metrics.snapshot(),
    })
}

fn usage_exhausted(message: &str) -> (StatusCode, Json<BridgeError>) {
    (
        StatusCode::PAYMENT_REQUIRED,
        Json(BridgeError {
            code: "usage_exhausted",
            disable_hooks: true,
            message: message.to_owned(),
        }),
    )
}

fn is_usage_exhausted(error: &CompactionError) -> bool {
    let CompactionError::TypeSafe(error) = error else {
        return false;
    };
    let Some(api) = error.api_error() else {
        return false;
    };
    if api.status == StatusCode::PAYMENT_REQUIRED.as_u16() {
        return true;
    }
    if api.status != StatusCode::FORBIDDEN.as_u16() {
        return false;
    }
    let message = api.message.to_ascii_lowercase();
    [
        "quota",
        "credit",
        "billing",
        "usage",
        "payment",
        "insufficient",
    ]
    .iter()
    .any(|word| message.contains(word))
}

#[cfg(test)]
mod tests {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use httpmock::{Method::POST, MockServer};
    use tower::ServiceExt;

    use super::*;

    #[derive(Deserialize)]
    struct BridgeResponse {
        stats: BridgeStats,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct BridgeStats {
        requests: usize,
        elapsed_ms: u128,
    }

    #[tokio::test]
    async fn bridge_reuses_the_sdk_and_returns_camel_case_json() {
        let upstream = MockServer::start_async().await;
        let mock = upstream
            .mock_async(|when, then| {
                when.method(POST).path("/v1/systemone");
                then.status(200).json_body(serde_json::json!({
                    "model": "jev-test",
                    "usage": {"input_tokens": 40, "output_tokens": 2},
                    "answers": {
                        "call_t1": {"type": "noul", "noul": 0.1},
                        "result_t1": {"type": "noul", "noul": 0.1}
                    }
                }));
            })
            .await;
        let client = Client::builder()
            .api_key("test")
            .base_url(upstream.base_url())
            .build()
            .unwrap();
        let request = Request::post("/compact")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "messages": [
                        {"role": "user", "text": "finish the fix", "toolUses": []},
                        {"role": "assistant", "text": "", "toolUses": [{
                            "tool_use_id": "one", "tool": "Read", "input": {"file_path": "old.rs"}
                        }]},
                        {"role": "user", "text": "", "toolUses": [], "toolResults": [{
                            "tool_use_id": "one", "text": "old output", "isError": false
                        }]}
                    ],
                    "options": {"preserveRecentMessages": 0}
                })
                .to_string(),
            ))
            .unwrap();

        let app = app(client, None);
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 1_000_000).await.unwrap();
        let output: BridgeResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(output.stats.requests, 1);
        assert!(output.stats.elapsed_ms < 10_000);

        let health = app
            .oneshot(Request::get("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = to_bytes(health.into_body(), 1_000_000).await.unwrap();
        let health: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(health["status"], "ready");
        assert_eq!(health["usage"]["attempts"], 1);
        assert_eq!(health["usage"]["succeeded"], 1);
        assert_eq!(health["usage"]["jevRequests"], 1);
        assert_eq!(health["usage"]["jevInputTokens"], 40);
        assert_eq!(health["usage"]["callsSeen"], 1);
        mock.assert_calls_async(1).await;
    }

    #[tokio::test]
    async fn usage_exhaustion_opens_the_circuit() {
        let upstream = MockServer::start_async().await;
        let mock = upstream
            .mock_async(|when, then| {
                when.method(POST).path("/v1/systemone");
                then.status(402)
                    .json_body(serde_json::json!({"error": "credits exhausted"}));
            })
            .await;
        let client = Client::builder()
            .api_key("test")
            .base_url(upstream.base_url())
            .build()
            .unwrap();
        let body = serde_json::json!({
            "messages": [
                {"role": "user", "text": "finish", "toolUses": []},
                {"role": "assistant", "text": "", "toolUses": [{
                    "tool_use_id": "one", "tool": "Read", "input": {}
                }]},
                {"role": "user", "text": "", "toolUses": [], "toolResults": [{
                    "tool_use_id": "one", "text": "old", "isError": false
                }]}
            ],
            "options": {"preserveRecentMessages": 0}
        })
        .to_string();
        let app = app(client, None);

        for _ in 0..2 {
            let request = Request::post("/compact")
                .header("content-type", "application/json")
                .body(Body::from(body.clone()))
                .unwrap();
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);
            let body = to_bytes(response.into_body(), 1_000_000).await.unwrap();
            let error: BridgeSignal = serde_json::from_slice(&body).unwrap();
            assert_eq!(error.code, "usage_exhausted");
            assert!(error.disable_hooks);
        }
        mock.assert_calls_async(1).await;
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct BridgeSignal {
        code: String,
        disable_hooks: bool,
    }
}
