use duckduckgo::browser::Browser;
use duckduckgo::response::LiteSearchResult;
use reqwest_011::Client;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use thiserror::Error;
use wasmer::{Instance, Module, Store};
use wasmer_wasix::{Pipe, WasiEnv, WasiFunctionEnv};

use crate::db::{self, WasiApp};
use crate::providers::{ChatAttachment, ChatKnowledgeBaseRef};

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct ToolInvocation {
    pub tool_name: String,
    pub query: String,
    pub collapsed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChatToolKind {
    DuckDuckGoSearch,
    ReadTextFile,
    WriteTextFile,
}

impl ChatToolKind {
    pub fn id(self) -> &'static str {
        match self {
            ChatToolKind::DuckDuckGoSearch => "duckduckgo_search",
            ChatToolKind::ReadTextFile => "read_text_file",
            ChatToolKind::WriteTextFile => "write_text_file",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ChatToolKind::DuckDuckGoSearch => "DuckDuckGo Search",
            ChatToolKind::ReadTextFile => "Read File",
            ChatToolKind::WriteTextFile => "Write File",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            ChatToolKind::DuckDuckGoSearch => "Search the web for recent pages and snippets.",
            ChatToolKind::ReadTextFile => {
                "Read the contents of a text file from the virtual filesystem."
            }
            ChatToolKind::WriteTextFile => {
                "Write text content to a file in the virtual filesystem."
            }
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            ChatToolKind::DuckDuckGoSearch => "Search",
            ChatToolKind::ReadTextFile => "File",
            ChatToolKind::WriteTextFile => "File",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatToolConfig {
    pub kind: ChatToolKind,
    pub wasi_app_id: Option<String>,
}

impl ChatToolConfig {
    pub fn new(kind: ChatToolKind) -> Self {
        Self {
            kind,
            wasi_app_id: None,
        }
    }

    pub fn new_wasi(app_id: &str) -> Self {
        Self {
            kind: ChatToolKind::DuckDuckGoSearch, // placeholder
            wasi_app_id: Some(app_id.to_string()),
        }
    }

    pub fn is_wasi_app(&self) -> bool {
        self.wasi_app_id.is_some()
    }

    pub fn matches_builtin_kind(&self, kind: ChatToolKind) -> bool {
        !self.is_wasi_app() && self.kind == kind
    }
}

pub const AVAILABLE_TOOLS: &[ChatToolKind] = &[
    ChatToolKind::DuckDuckGoSearch,
    ChatToolKind::ReadTextFile,
    ChatToolKind::WriteTextFile,
];

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct VirtualFs {
    pub chat_id: String,
    pub files: HashMap<String, String>,
}

impl VirtualFs {
    pub fn read_text_file(&self, path: &str) -> Result<String, String> {
        if let Some(content) = self.files.get(path) {
            return Ok(content.clone());
        }

        let disk_path = db::chat_vfs_root(&self.chat_id).join(path.trim_start_matches('/'));
        if disk_path.exists() {
            return std::fs::read_to_string(disk_path).map_err(|err| err.to_string());
        }

        self.files
            .get(path)
            .cloned()
            .ok_or_else(|| format!("File not found: {}", path))
    }

    pub fn write_text_file(&mut self, path: &str, content: &str) -> Result<(), String> {
        self.files.insert(path.to_string(), content.to_string());
        db::upsert_chat_vfs_text_file(&self.chat_id, path, content)
            .map_err(|err| err.to_string())?;
        Ok(())
    }

    pub fn list_files(&self) -> Vec<String> {
        let mut files: Vec<String> = self.files.keys().cloned().collect();
        files.extend(db::list_chat_vfs_paths(&self.chat_id));
        files.sort();
        files.dedup();
        files
    }
}

pub type VfsHandle = Arc<Mutex<VirtualFs>>;

pub fn new_vfs() -> VfsHandle {
    Arc::new(Mutex::new(VirtualFs::default()))
}

pub fn vfs_from_json(chat_id: &str, json: &str) -> VfsHandle {
    let mut vfs: VirtualFs = serde_json::from_str(json).unwrap_or_default();
    vfs.chat_id = chat_id.to_string();
    Arc::new(Mutex::new(vfs))
}

pub fn vfs_to_json(vfs: &VfsHandle) -> String {
    let guard = vfs.lock().unwrap();
    serde_json::to_string(&*guard).unwrap_or_else(|_| "{}".to_string())
}

pub fn parse_tool_configs(json: &str) -> Vec<ChatToolConfig> {
    serde_json::from_str(json).unwrap_or_default()
}

pub fn serialize_tool_configs(configs: &[ChatToolConfig]) -> String {
    serde_json::to_string(configs).unwrap_or_else(|_| "[]".to_string())
}

pub fn has_tool(tools: &[ChatToolConfig], kind: ChatToolKind) -> bool {
    tools.iter().any(|tool| tool.matches_builtin_kind(kind))
}

pub fn has_wasi_tool(tools: &[ChatToolConfig]) -> bool {
    tools.iter().any(|tool| tool.wasi_app_id.is_some())
}

pub fn build_agent_preamble(
    system_prompt: &str,
    model: &str,
    active_tools: &[ChatToolConfig],
    wasi_apps: &[WasiApp],
    attachments: &[ChatAttachment],
    knowledge_bases: &[ChatKnowledgeBaseRef],
    retrieved_context: &str,
) -> String {
    let base = if system_prompt.trim().is_empty() {
        "You are a helpful assistant.".to_string()
    } else {
        system_prompt.trim().to_string()
    };

    let mut notes: Vec<String> = Vec::new();

    notes.push(format!(
        "You are the assistant running on the Ollama model `{model}`. Do not claim to be a different model family, vendor, or architecture. If asked what model you are, say `{model}`."
    ));
    notes.push(
        "Do not claim you have bash, shell, terminal, OS, or general system access unless that exact capability is explicitly listed in the enabled tools below. If asked what tools you have, list only the enabled tools below.".to_string()
    );

    let mut inventory: Vec<String> = Vec::new();

    if has_tool(active_tools, ChatToolKind::DuckDuckGoSearch) {
        inventory.push(
            "- `duckduckgo_search(query)`: search the web for current results and snippets."
                .to_string(),
        );
    }

    if has_tool(active_tools, ChatToolKind::ReadTextFile) {
        inventory.push(
            "- `read_text_file(path)`: read a text file from the virtual filesystem.".to_string(),
        );
    }

    if has_tool(active_tools, ChatToolKind::WriteTextFile) {
        inventory.push(
            "- `write_text_file(path, content)`: write a text file to the virtual filesystem."
                .to_string(),
        );
    }

    for app_id in active_tools
        .iter()
        .filter_map(|tool| tool.wasi_app_id.as_deref())
    {
        if let Some(app) = wasi_apps.iter().find(|app| app.id == app_id) {
            let tool_name = WasiAppTool::normalize_tool_name(&app.name);
            let summary = if app.description.trim().is_empty() {
                format!("run the `{}` WASM tool", app.name)
            } else {
                app.description.trim().to_string()
            };
            inventory.push(format!(
                "- `{tool_name}(args)`: {summary}. User-facing name: `{}`. Pass CLI arguments as one string in `args`; do not mention bash unless this tool's own help explicitly says it provides a shell.",
                app.name
            ));
        }
    }

    if !inventory.is_empty() {
        notes.push(format!(
            "Enabled tools in this chat:\n{}",
            inventory.join("\n")
        ));
        notes.push(
            "When the user asks what tools you have, answer from that exact list with those exact names. Do not invent extra capabilities or hidden tools.".to_string()
        );
    }

    if has_tool(active_tools, ChatToolKind::DuckDuckGoSearch) {
        notes.push(
            "When the task requires current web information, call `duckduckgo_search` instead of guessing. Never pretend you used a tool if you did not.".to_string()
        );
        notes.push(
            "If you used DuckDuckGo Search, briefly list the searches at the end in the format: [Searches: query1, query2, ...]".to_string()
        );
    }

    if has_wasi_tool(active_tools) {
        notes.push(
            "WASM-backed tools are normal callable tools. Use their exact tool names from the enabled-tools list, and pass CLI arguments in `args` without repeating the tool name.".to_string()
        );
    }

    if has_tool(active_tools, ChatToolKind::ReadTextFile)
        || has_tool(active_tools, ChatToolKind::WriteTextFile)
    {
        notes.push(
            "If file tools are available, you can read and write files through those tools. Do not claim file access is unavailable when those tools are enabled.".to_string()
        );
    }

    if !attachments.is_empty() {
        let attachment_lines = attachments
            .iter()
            .map(|attachment| {
                if attachment.inline_context.is_empty() {
                    format!("- `{}` at `{}`", attachment.name, attachment.path)
                } else {
                    format!(
                        "- `{}` at `{}` (short text already included in context)",
                        attachment.name, attachment.path
                    )
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        notes.push(format!("Files available in this chat:\n{attachment_lines}"));
    }

    if !knowledge_bases.is_empty() {
        let lines = knowledge_bases
            .iter()
            .map(|kb| {
                if kb.description.trim().is_empty() {
                    format!("- `{}`", kb.name)
                } else {
                    format!("- `{}`: {}", kb.name, kb.description.trim())
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        notes.push(format!("Attached knowledge bases:\n{lines}"));
    }

    let inline_texts = attachments
        .iter()
        .filter(|attachment| !attachment.inline_context.is_empty())
        .map(|attachment| {
            format!(
                "[File: {} at {}]\n{}",
                attachment.name, attachment.path, attachment.inline_context
            )
        })
        .collect::<Vec<_>>();
    if !inline_texts.is_empty() {
        notes.push(format!(
            "Short text files uploaded to this chat are part of the active context:\n{}",
            inline_texts.join("\n\n")
        ));
    }

    if !retrieved_context.trim().is_empty() {
        notes.push(format!(
            "Retrieved long-document context for the current request:\n{}",
            retrieved_context.trim()
        ));
        notes.push(
            "When the retrieved context answers the question, ground your answer in it and mention the relevant file names. If the retrieval is insufficient, say so instead of inventing details.".to_string()
        );
    }

    if notes.is_empty() {
        base
    } else {
        format!("{base}\n\n{}", notes.join("\n\n"))
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
            description: "Search DuckDuckGo for current web results and short snippets."
                .to_string(),
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

    let mut lines = vec![format!("DuckDuckGo search results for '{query}':")];

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

pub fn parse_tool_invocations(response: &str) -> (String, Vec<ToolInvocation>) {
    let search_patterns = ["[Searches:", "[Search:", "Searches:", "Search:"];

    for search_pattern in &search_patterns {
        if let Some(start_idx) = response.to_lowercase().find(&search_pattern.to_lowercase()) {
            let before_searches = response[..start_idx].trim();

            let rest = &response[start_idx..];
            if let Some(end_bracket) = rest.find(']') {
                let searches_part = &rest[search_pattern.len()..end_bracket];

                let queries: Vec<ToolInvocation> = searches_part
                    .split(',')
                    .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
                    .filter(|s| !s.is_empty())
                    .map(|query| ToolInvocation {
                        tool_name: "DuckDuckGo Search".to_string(),
                        query,
                        collapsed: false,
                    })
                    .collect();

                if !queries.is_empty() {
                    return (before_searches.to_string(), queries);
                }
            } else {
                let queries: Vec<ToolInvocation> = search_patterns
                    .iter()
                    .flat_map(|p| rest.split(p))
                    .skip(1)
                    .next()
                    .map(|s| {
                        s.trim()
                            .split(',')
                            .map(|q| q.trim().trim_matches('"').trim_matches('\'').to_string())
                            .filter(|q| !q.is_empty())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
                    .into_iter()
                    .map(|query| ToolInvocation {
                        tool_name: "DuckDuckGo Search".to_string(),
                        query,
                        collapsed: false,
                    })
                    .collect();

                if !queries.is_empty() {
                    return (before_searches.to_string(), queries);
                }
            }
        }
    }

    (response.to_string(), Vec::new())
}

// ── Read Text File Tool ─────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ReadTextFileArgs {
    pub path: String,
}

#[derive(Debug, Error)]
#[error("{0}")]
pub struct ReadTextFileError(pub String);

#[derive(Clone)]
pub struct ReadTextFileTool {
    vfs: VfsHandle,
}

impl ReadTextFileTool {
    pub fn new(vfs: VfsHandle) -> Self {
        Self { vfs }
    }
}

impl Tool for ReadTextFileTool {
    const NAME: &'static str = "read_text_file";

    type Error = ReadTextFileError;
    type Args = ReadTextFileArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Read the contents of a text file from the virtual filesystem."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The path to the file to read"
                    }
                },
                "required": ["path"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let vfs = self
            .vfs
            .lock()
            .map_err(|e| ReadTextFileError(e.to_string()))?;
        vfs.read_text_file(&args.path).map_err(ReadTextFileError)
    }
}

// ── Write Text File Tool ───────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct WriteTextFileArgs {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Error)]
#[error("{0}")]
pub struct WriteTextFileError(pub String);

#[derive(Clone)]
pub struct WriteTextFileTool {
    vfs: VfsHandle,
}

impl WriteTextFileTool {
    pub fn new(vfs: VfsHandle) -> Self {
        Self { vfs }
    }
}

impl Tool for WriteTextFileTool {
    const NAME: &'static str = "write_text_file";

    type Error = WriteTextFileError;
    type Args = WriteTextFileArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Write text content to a file in the virtual filesystem.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The path to the file to write"
                    },
                    "content": {
                        "type": "string",
                        "description": "The text content to write to the file"
                    }
                },
                "required": ["path", "content"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let mut vfs = self
            .vfs
            .lock()
            .map_err(|e| WriteTextFileError(e.to_string()))?;
        vfs.write_text_file(&args.path, &args.content)
            .map_err(WriteTextFileError)?;
        Ok(format!("Successfully wrote to {}", args.path))
    }
}

// ── WASI App Tool ─────────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
pub struct WasiAppArgs {
    #[serde(default)]
    pub args: String,
}

#[derive(Debug, Error)]
#[error("{0}")]
pub struct WasiAppError(pub String);

#[derive(Clone)]
pub struct WasiAppTool {
    pub name: String,
    pub description: String,
    pub help_text: String,
    pub module_path: String,
    pub vfs_root: PathBuf,
}

impl WasiAppTool {
    pub fn new(
        name: &str,
        description: &str,
        help_text: &str,
        module_path: String,
        vfs_root: PathBuf,
    ) -> Self {
        Self {
            name: name.to_string(),
            description: description.to_string(),
            help_text: help_text.to_string(),
            module_path,
            vfs_root,
        }
    }

    pub fn normalize_tool_name(name: &str) -> String {
        let mut out = String::new();
        let mut last_was_sep = false;

        for ch in name.chars() {
            if ch.is_ascii_alphanumeric() {
                out.push(ch.to_ascii_lowercase());
                last_was_sep = false;
            } else if !last_was_sep && !out.is_empty() {
                out.push('_');
                last_was_sep = true;
            }
        }

        let out = out.trim_matches('_').to_string();
        if out.is_empty() {
            return "tool".to_string();
        }

        if out.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
            format!("tool_{out}")
        } else {
            out
        }
    }

    pub fn tool_name(&self) -> String {
        Self::normalize_tool_name(&self.name)
    }

    fn tool_description(&self) -> String {
        let summary = if self.description.trim().is_empty() {
            format!("Run the `{}` command line tool.", self.name)
        } else {
            self.description.trim().to_string()
        };

        let usage = format!(
            "Call this tool by passing the command line arguments as a single shell-style string in `args`. Do not repeat the tool name. Use an empty string when no arguments are needed."
        );

        let help = self.help_text.trim();
        if help.is_empty() || help == "No help available" {
            format!("{summary}\n\n{usage}")
        } else {
            format!("{summary}\n\n{usage}\n\nCLI help:\n{help}")
        }
    }
}

impl Tool for WasiAppTool {
    const NAME: &'static str = "wasi_app";

    type Error = WasiAppError;
    type Args = WasiAppArgs;
    type Output = String;

    fn name(&self) -> String {
        self.tool_name()
    }

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: self.tool_name(),
            description: self.tool_description(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "args": {
                        "type": "string",
                        "description": format!("Arguments for `{}` as one shell-style string, excluding the tool name itself. Example: `--help` or `input.txt --format json`.", self.name)
                    }
                }
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        run_wasi_cli(
            &self.module_path,
            &self.name,
            &args.args,
            Some(&self.vfs_root),
        )
        .await
        .map_err(WasiAppError)
    }
}

async fn run_wasi_cli(
    module_path: &str,
    module_name: &str,
    cli_args: &str,
    _vfs_root: Option<&PathBuf>,
) -> Result<String, String> {
    let (stdout_tx, mut stdout_rx) = Pipe::channel();
    let (stderr_tx, mut stderr_rx) = Pipe::channel();

    let args_vec: Vec<String> = std::iter::once(module_name.to_string())
        .chain(cli_args.split_whitespace().map(String::from))
        .collect();

    let mut store = Store::default();
    let engine = store.engine().clone();
    let wasm_bytes =
        db::read_storage_bytes(module_path).map_err(|e| format!("Failed to read wasm: {e}"))?;
    let module =
        Module::new(&store, wasm_bytes).map_err(|e| format!("Failed to compile wasm: {e}"))?;

    let mut wasi_env_builder = WasiEnv::builder(module_name)
        .args(&args_vec)
        .stdout(Box::new(stdout_tx))
        .stderr(Box::new(stderr_tx));
    
    // Don't mount real directories - let WASI use its virtual filesystem
    wasi_env_builder.set_engine(engine);

    let wasi_env = wasi_env_builder
        .build()
        .map_err(|e| format!("WASI env build error: {e}"))?;

    let mut func_env = WasiFunctionEnv::new(&mut store, wasi_env);
    let imports = func_env
        .import_object(&mut store, &module)
        .map_err(|e| format!("Failed to build WASI imports: {e}"))?;

    let instance = Instance::new(&mut store, &module, &imports)
        .map_err(|e| format!("Failed to instantiate wasm: {e}"))?;

    func_env
        .initialize(&mut store, instance.clone())
        .map_err(|e| format!("Failed to initialize WASI env: {e}"))?;

    let start = instance
        .exports
        .get_function("_start")
        .or_else(|_| instance.exports.get_function("main"));

    if let Ok(start) = start {
        start
            .call(&mut store, &[])
            .map_err(|e| format!("WASI execution error: {e}"))?;
    }

    drop(instance);
    drop(func_env);
    drop(store);

    let mut output = String::new();
    tokio::io::AsyncReadExt::read_to_string(&mut stdout_rx, &mut output)
        .await
        .map_err(|e| format!("stdout read: {e}"))?;

    let mut stderr_out = String::new();
    let _ = tokio::io::AsyncReadExt::read_to_string(&mut stderr_rx, &mut stderr_out).await;
    if !stderr_out.trim().is_empty() {
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str(&format!("---\nstderr:\n{}", stderr_out.trim()));
    }

    if output.is_empty() {
        output = "(no output)".to_string();
    }

    Ok(output)
}

pub async fn generate_help_text(module_path: &str, module_name: &str) -> String {
    for help_arg in &["-h", "--help", "help"] {
        let result = run_wasi_cli(module_path, module_name, help_arg, None).await;
        if let Ok(output) = result {
            if !output.contains("unrecognized")
                && !output.contains("unknown")
                && !output.contains("invalid")
            {
                return output;
            }
        }
    }
    "No help available".to_string()
}
