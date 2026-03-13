//! Ollama provider — streams responses from a local Ollama instance.
//!
//! Uses the `/api/chat` endpoint with `"stream": true`. Ollama sends one
//! JSON object per line. HTTP chunks do NOT align to line boundaries, so
//! we maintain a byte buffer and only parse complete lines.
//!
//! Model listing uses `/api/tags`.

use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use super::{Message, ProviderError, RemoteModel};

/// Default Ollama base URL — can be overridden at runtime.
pub const DEFAULT_BASE_URL: &str = "http://localhost:11434";

// ── Wire types ────────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<OllamaMessage>,
    stream: bool,
}

#[derive(Serialize)]
struct OllamaMessage {
    role: String,
    content: String,
}

impl From<&Message> for OllamaMessage {
    fn from(m: &Message) -> Self {
        OllamaMessage {
            role: m.role.clone(),
            content: m.content.clone(),
        }
    }
}

#[derive(Deserialize)]
struct ChatChunk {
    message: ChunkMessage,
    #[serde(default)]
    done: bool,
}

#[derive(Deserialize)]
struct ChunkMessage {
    content: String,
}

#[derive(Deserialize)]
struct TagsResponse {
    models: Vec<TagModel>,
}

#[derive(Deserialize)]
struct TagModel {
    name: String,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Fetch the list of locally available models from Ollama.
pub async fn list_models(base_url: &str) -> Result<Vec<RemoteModel>, ProviderError> {
    let url = format!("{base_url}/api/tags");
    let resp = Client::new()
        .get(&url)
        .send()
        .await
        .map_err(|e| ProviderError::Http(e.to_string()))?;

    if !resp.status().is_success() {
        return Err(ProviderError::Http(format!(
            "Ollama /api/tags returned {}",
            resp.status()
        )));
    }

    let tags: TagsResponse = resp
        .json()
        .await
        .map_err(|e| ProviderError::Parse(e.to_string()))?;

    Ok(tags
        .models
        .into_iter()
        .map(|m| RemoteModel {
            label: m.name.clone(),
            id: m.name,
        })
        .collect())
}

/// Stream a chat completion from Ollama.
///
/// Spawns the HTTP request + stream reading on a Tokio task and sends
/// each token chunk through `tx`. The channel closes when the task
/// finishes (after `done: true` or on error).
///
/// **Line buffering:** HTTP chunks from Ollama don't align to JSON-line
/// boundaries. We accumulate bytes in a `Vec<u8>` and only parse when
/// we see a `\n`, ensuring we never try to decode a partial JSON object.
pub fn chat_stream(
    base_url: String,
    model: String,
    messages: Vec<Message>,
) -> mpsc::Receiver<Result<String, ProviderError>> {
    let (tx, rx) = mpsc::channel::<Result<String, ProviderError>>(128);

    tokio::spawn(async move {
        let ollama_msgs: Vec<OllamaMessage> = messages.iter().map(OllamaMessage::from).collect();
        let body = ChatRequest {
            model,
            messages: ollama_msgs,
            stream: true,
        };

        let resp = match Client::new()
            .post(format!("{base_url}/api/chat"))
            .json(&body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                let _ = tx.send(Err(ProviderError::Http(e.to_string()))).await;
                return;
            }
        };

        if !resp.status().is_success() {
            let status = resp.status().to_string();
            let body_text = resp.text().await.unwrap_or_default();
            let _ = tx
                .send(Err(ProviderError::Http(format!("{status}: {body_text}"))))
                .await;
            return;
        }

        let mut stream = resp.bytes_stream();
        // Line buffer — accumulates bytes until we see '\n'.
        let mut line_buf: Vec<u8> = Vec::with_capacity(256);

        while let Some(chunk) = stream.next().await {
            let bytes = match chunk {
                Ok(b) => b,
                Err(e) => {
                    let _ = tx.send(Err(ProviderError::Io(e.to_string()))).await;
                    return;
                }
            };

            for byte in bytes.iter().copied() {
                if byte == b'\n' {
                    // We have a complete line — try to parse it.
                    let line = std::mem::take(&mut line_buf);
                    if line.is_empty() {
                        continue;
                    }
                    match serde_json::from_slice::<ChatChunk>(&line) {
                        Ok(obj) => {
                            if !obj.message.content.is_empty() {
                                if tx.send(Ok(obj.message.content)).await.is_err() {
                                    return; // receiver dropped
                                }
                            }
                            if obj.done {
                                return;
                            }
                        }
                        Err(e) => {
                            let _ = tx
                                .send(Err(ProviderError::Parse(format!(
                                    "{e}: {}",
                                    String::from_utf8_lossy(&line)
                                ))))
                                .await;
                            return;
                        }
                    }
                } else {
                    line_buf.push(byte);
                }
            }
        }

        // Flush any remaining bytes in the buffer (should be empty for well-formed streams).
        if !line_buf.is_empty() {
            if let Ok(obj) = serde_json::from_slice::<ChatChunk>(&line_buf) {
                if !obj.message.content.is_empty() {
                    let _ = tx.send(Ok(obj.message.content)).await;
                }
            }
        }
    });

    rx
}
