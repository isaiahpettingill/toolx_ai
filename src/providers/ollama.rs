//! Ollama provider powered by Rig.

use reqwest::Client;
use serde::Deserialize;

use futures_util::StreamExt;
use rig::agent::MultiTurnStreamItem;
use rig::client::CompletionClient;
use rig::client::Nothing;
use rig::completion::message::Message as RigMessage;
use rig::providers::ollama as rig_ollama;
use rig::streaming::{StreamedAssistantContent, StreamingChat};
use rig::tool::ToolDyn;
use tokio::sync::mpsc;

use super::{ChatAttachment, ChatKnowledgeBaseRef, Message, ProviderError, RemoteModel};
use crate::db::MessageCitation;
use crate::db::{self, WasiApp};
use crate::rag;
use crate::tools::{
    self, ChatToolConfig, ChatToolKind, DuckDuckGoSearchTool, ReadTextFileTool, ToolInvocation,
    VfsHandle, WasiAppTool, WriteTextFileTool,
};

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

/// Known embeddings model name patterns
const EMBEDDING_PATTERNS: &[&str] = &[
    "embed",
    "nomic",
    "mxbai",
    "all-minilm",
    "bge-",
    "e5-",
    "gte-",
];

/// Fetch embeddings models from Ollama - returns static list filtered to known patterns if available
pub async fn list_embedding_models(base_url: &str) -> Vec<String> {
    let tags = match list_models(base_url).await {
        Ok(models) => models,
        Err(_) => return rag::default_embedding_models(),
    };

    let embedding_models: Vec<String> = tags
        .into_iter()
        .filter(|m| {
            let name_lower = m.id.to_lowercase();
            EMBEDDING_PATTERNS.iter().any(|p| name_lower.contains(p))
        })
        .map(|m| m.id)
        .collect();

    if embedding_models.is_empty() {
        rag::default_embedding_models()
    } else {
        embedding_models
    }
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

pub fn chat_stream(
    base_url: String,
    model: String,
    system_prompt: String,
    messages: Vec<Message>,
    prompt: String,
    active_tools: Vec<ChatToolConfig>,
    wasi_apps: Vec<WasiApp>,
    vfs: VfsHandle,
    attachments: Vec<ChatAttachment>,
    knowledge_bases: Vec<ChatKnowledgeBaseRef>,
    retrieved_context: String,
) -> mpsc::Receiver<Result<StreamChunk, ProviderError>> {
    let (tx, rx) = mpsc::channel(64);

    tokio::spawn(async move {
        let result = run_chat_stream(
            base_url,
            model,
            system_prompt,
            messages,
            prompt,
            active_tools,
            wasi_apps,
            vfs,
            attachments,
            knowledge_bases,
            retrieved_context,
            tx.clone(),
        )
        .await;

        if let Err(err) = result {
            let _ = tx.send(Err(err)).await;
        }
    });

    rx
}

#[derive(Debug, Clone)]
pub struct StreamChunk {
    pub delta: String,
    pub final_content: Option<String>,
    pub tool_invocations: Option<Vec<ToolInvocation>>,
    pub citations: Option<Vec<MessageCitation>>,
}


async fn run_chat_stream(
    base_url: String,
    model: String,
    system_prompt: String,
    messages: Vec<Message>,
    prompt: String,
    active_tools: Vec<ChatToolConfig>,
    wasi_apps: Vec<WasiApp>,
    vfs: VfsHandle,
    attachments: Vec<ChatAttachment>,
    knowledge_bases: Vec<ChatKnowledgeBaseRef>,
    retrieved_context: String,
    tx: mpsc::Sender<Result<StreamChunk, ProviderError>>,
) -> Result<(), ProviderError> {
    let client = rig_ollama::Client::builder()
        .api_key(Nothing)
        .base_url(base_url)
        .build()
        .map_err(|e| ProviderError::Http(e.to_string()))?;

    let preamble = tools::build_agent_preamble(
        &system_prompt,
        &model,
        &active_tools,
        &wasi_apps,
        &attachments,
        &knowledge_bases,
        &retrieved_context,
    );
    let history = messages
        .into_iter()
        .map(into_rig_message)
        .collect::<Vec<_>>();
    let vfs_root = {
        let guard = vfs
            .lock()
            .map_err(|err| ProviderError::Io(err.to_string()))?;
        db::chat_vfs_root(&guard.chat_id)
    };

    eprintln!("[DEBUG] Tools enabled: {:?}", active_tools);
    eprintln!("[DEBUG] WASI apps: {:?}", wasi_apps.len());
    eprintln!("[DEBUG] Preamble: {}", preamble);

    let has_wasi = tools::has_wasi_tool(&active_tools);
    let has_ddg = tools::has_tool(&active_tools, ChatToolKind::DuckDuckGoSearch);
    let wasi_app_ids: Vec<String> = active_tools
        .iter()
        .filter_map(|t| t.wasi_app_id.clone())
        .collect();

    let mut all_tools: Vec<Box<dyn ToolDyn>> = Vec::new();
    if has_wasi {
        all_tools.push(Box::new(ReadTextFileTool::new(vfs.clone())));
        all_tools.push(Box::new(WriteTextFileTool::new(vfs.clone())));
    }
    if has_ddg {
        all_tools.push(Box::new(DuckDuckGoSearchTool));
    }
    all_tools.extend(
        wasi_apps
            .iter()
            .filter(|app| wasi_app_ids.contains(&app.id))
            .map(|app| {
                Box::new(WasiAppTool::new(
                    &app.name,
                    &app.description,
                    &app.help_text,
                    app.file_path.clone(),
                    vfs_root.clone(),
                )) as Box<dyn ToolDyn>
            }),
    );

    let max_turns = if !wasi_app_ids.is_empty() || has_ddg {
        6
    } else if has_wasi {
        4
    } else {
        2
    };

    let mut stream = client
        .agent(&model)
        .preamble(&preamble)
        .tools(all_tools)
        .default_max_turns(max_turns)
        .build()
        .stream_chat(prompt, history)
        .multi_turn(max_turns)
        .await;

    let mut accumulated = String::new();
    let mut final_response: Option<String> = None;

    while let Some(item) = stream.next().await {
        match item {
            Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Text(text))) => {
                accumulated.push_str(&text.text);
                tx.send(Ok(StreamChunk {
                    delta: text.text,
                    final_content: None,
                    tool_invocations: None,
                    citations: None,
                }))
                .await
                .ok();
            }
            Ok(MultiTurnStreamItem::FinalResponse(res)) => {
                final_response = Some(res.response().to_string());
            }
            Ok(_) => {}
            Err(e) => {
                return Err(ProviderError::Parse(e.to_string()));
            }
        }
    }

    let resolved = final_response.unwrap_or_else(|| accumulated.clone());
    let (clean_content, tool_invocations) = tools::parse_tool_invocations(&resolved);
    eprintln!("[DEBUG] Parsed invocations: {:?}", tool_invocations);

    tx.send(Ok(StreamChunk {
        delta: String::new(),
        final_content: Some(clean_content),
        tool_invocations: Some(tool_invocations),
        citations: None,
    }))
    .await
    .ok();

    Ok(())
}

fn into_rig_message(message: Message) -> RigMessage {
    match message.role.as_str() {
        "assistant" => RigMessage::assistant(message.content),
        _ => RigMessage::user(message.content),
    }
}
