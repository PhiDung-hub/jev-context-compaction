use jev_context_compaction::{CompactOptions, CompactionError, Message, compact};
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use typesafe_ai::Client;

#[derive(Deserialize)]
struct Input {
    messages: Vec<Message>,
    #[serde(default)]
    options: CompactOptions,
}

#[tokio::main]
async fn main() -> Result<(), CompactionError> {
    let mut bytes = Vec::new();
    tokio::io::stdin().read_to_end(&mut bytes).await?;
    let input: Input = serde_json::from_slice(&bytes)?;
    let result = compact(&Client::from_env()?, &input.messages, &input.options).await?;
    let encoded = serde_json::to_vec(&result)?;
    let mut output = tokio::io::stdout();
    output.write_all(&encoded).await?;
    output.write_all(b"\n").await?;
    Ok(())
}
