use duckduckgo::browser::Browser;
use duckduckgo::response::LiteSearchResult;
use reqwest_011::Client;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChatToolKind {
    DuckDuckGoSearch,
}

impl ChatToolKind {
    pub fn id(self) -> &'static str {
        match self {
            ChatToolKind::DuckDuckGoSearch => "duckduckgo_search",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ChatToolKind::DuckDuckGoSearch => "DuckDuckGo Search",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            ChatToolKind::DuckDuckGoSearch => "Search the web for recent pages and snippets.",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            ChatToolKind::DuckDuckGoSearch => "Search",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatToolConfig {
    pub kind: ChatToolKind,
}

impl ChatToolConfig {
    pub fn new(kind: ChatToolKind) -> Self {
        Self { kind }
    }
}

pub const AVAILABLE_TOOLS: &[ChatToolKind] = &[ChatToolKind::DuckDuckGoSearch];

pub fn parse_tool_configs(json: &str) -> Vec<ChatToolConfig> {
    serde_json::from_str(json).unwrap_or_default()
}

pub fn serialize_tool_configs(configs: &[ChatToolConfig]) -> String {
    serde_json::to_string(configs).unwrap_or_else(|_| "[]".to_string())
}

pub fn has_tool(tools: &[ChatToolConfig], kind: ChatToolKind) -> bool {
    tools.iter().any(|tool| tool.kind == kind)
}

pub fn build_agent_preamble(system_prompt: &str, active_tools: &[ChatToolConfig]) -> String {
    let base = if system_prompt.trim().is_empty() {
        "You are a helpful assistant.".to_string()
    } else {
        system_prompt.trim().to_string()
    };

    if has_tool(active_tools, ChatToolKind::DuckDuckGoSearch) {
        format!(
            "{base}\n\nWhen the user needs fresh web information, local recommendations, live availability, recent events, or facts you are not confident about, call the DuckDuckGo Search tool. Never pretend you searched if you did not use the tool."
        )
    } else {
        base
    }
}

#[derive(Debug, Deserialize)]
pub struct DuckDuckGoSearchArgs {
    pub query: String,
}

#[derive(Debug, Error)]
#[error("{0}")]
pub struct DuckDuckGoToolError(pub String);

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct DuckDuckGoSearchTool;

impl Tool for DuckDuckGoSearchTool {
    const NAME: &'static str = "duckduckgo_search";

    type Error = DuckDuckGoToolError;
    type Args = DuckDuckGoSearchArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Search DuckDuckGo for current web results and short snippets.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The web search query to run"
                    }
                },
                "required": ["query"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        run_duckduckgo_search(&args.query)
            .await
            .map_err(DuckDuckGoToolError)
    }
}

async fn run_duckduckgo_search(query: &str) -> Result<String, String> {
    let client = Client::builder()
        .cookie_store(true)
        .build()
        .map_err(|e| format!("Failed to build DuckDuckGo client: {e}"))?;

    let browser = Browser::new(client);
    let results = browser
        .lite_search(query, "wt-wt", Some(5), "Mozilla/5.0")
        .await
        .map_err(|e| format!("DuckDuckGo search failed: {e}"))?;

    Ok(format_duckduckgo_results(query, &results))
}

fn format_duckduckgo_results(query: &str, results: &[LiteSearchResult]) -> String {
    if results.is_empty() {
        return format!("DuckDuckGo search for '{query}' returned no results.");
    }

    let mut lines = vec![format!(
        "DuckDuckGo search results for '{query}':"
    )];

    for (index, result) in results.iter().enumerate() {
        let title = result.title.trim();
        let url = result.url.trim();
        let snippet = result.snippet.trim();

        if snippet.is_empty() {
            lines.push(format!("{}. {}\n   {}", index + 1, title, url));
        } else {
            lines.push(format!(
                "{}. {}\n   {}\n   {}",
                index + 1,
                title,
                url,
                snippet
            ));
        }
    }

    lines.join("\n")
}
