//! Provider abstraction — each provider implements `chat_stream`.
//!
//! A provider receives the full message history and streams back
//! token chunks via a `tokio::sync::mpsc` channel. The UI reads
//! from the receiver and appends tokens to the assistant bubble in
//! real time.

pub mod ollama;
pub mod wasi;

use serde::{Deserialize, Serialize};

/// A single message in a conversation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
    pub role: String,
    pub content: String,
}

/// Errors returned by a provider.
#[derive(Debug, Clone)]
pub enum ProviderError {
    Http(String),
    Parse(String),
    Io(String),
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProviderError::Http(s) => write!(f, "HTTP error: {s}"),
            ProviderError::Parse(s) => write!(f, "Parse error: {s}"),
            ProviderError::Io(s) => write!(f, "IO error: {s}"),
        }
    }
}

/// A model entry returned from a provider's model listing.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteModel {
    pub id: String,
    pub label: String,
}
