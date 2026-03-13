//! Ollama provider powered by Rig.

use reqwest::Client;
use serde::Deserialize;

use rig::client::CompletionClient;
use rig::client::Nothing;
use rig::completion::message::Message as RigMessage;
use rig::completion::Chat;
use rig::providers::ollama as rig_ollama;

use super::{Message, ProviderError, RemoteModel};
use crate::tools::{self, ChatToolConfig, ChatToolKind, DuckDuckGoSearchTool, ToolInvocation};

/// Default Ollama base URL — can be overridden at runtime.
pub const DEFAULT_BASE_URL: &str = "http://localhost:11434";

#[derive(Deserialize)]
struct TagsResponse {
    models: Vec<TagModel>,
}

#[derive(Deserialize)]
struct TagModel {
    name: String,
}

// ── Public API ───────────────────────────────────────────────────────────────

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

/// Chat result that includes tool invocations
pub struct ChatResult {
    pub content: String,
    pub tool_invocations: Vec<ToolInvocation>,
}

pub async fn chat(
    base_url: String,
    model: String,
    system_prompt: String,
    messages: Vec<Message>,
    prompt: String,
    active_tools: Vec<ChatToolConfig>,
) -> Result<ChatResult, ProviderError> {
    let client = rig_ollama::Client::builder()
        .api_key(Nothing)
        .base_url(base_url)
        .build()
        .map_err(|e| ProviderError::Http(e.to_string()))?;

    let preamble = tools::build_agent_preamble(&system_prompt, &active_tools);
    let history = messages.into_iter().map(into_rig_message).collect::<Vec<_>>();

    eprintln!("[DEBUG] Tools enabled: {:?}", active_tools);
    eprintln!("[DEBUG] Preamble: {}", preamble);

    let content = if tools::has_tool(&active_tools, ChatToolKind::DuckDuckGoSearch) {
        eprintln!("[DEBUG] Adding DuckDuckGo search tool to agent");
        client
            .agent(&model)
            .preamble(&preamble)
            .default_max_turns(4)
            .tool(DuckDuckGoSearchTool)
            .build()
            .chat(prompt, history)
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))?
    } else {
        client
            .agent(&model)
            .preamble(&preamble)
            .default_max_turns(2)
            .build()
            .chat(prompt, history)
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))?
    };

    // Parse tool invocations from the response
    let (clean_content, tool_invocations) = tools::parse_tool_invocations(&content);
    eprintln!("[DEBUG] Parsed invocations: {:?}", tool_invocations);

    Ok(ChatResult {
        content: clean_content,
        tool_invocations,
    })
}

fn into_rig_message(message: Message) -> RigMessage {
    match message.role.as_str() {
        "assistant" => RigMessage::assistant(message.content),
        _ => RigMessage::user(message.content),
    }
}
