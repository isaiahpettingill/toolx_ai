use duckduckgo::browser::Browser;
use duckduckgo::response::LiteSearchResult;
use reqwest_011::Client;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use thiserror::Error;
use wasmer::{Instance, Module, Store};
use wasmer_wasix::{Pipe, WasiEnv, WasiFunctionEnv};

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
            ChatToolKind::ReadTextFile => "Read the contents of a text file from the virtual filesystem.",
            ChatToolKind::WriteTextFile => "Write text content to a file in the virtual filesystem.",
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
        Self { kind, wasi_app_id: None }
    }
    
    pub fn new_wasi(app_id: &str) -> Self {
        Self {
            kind: ChatToolKind::DuckDuckGoSearch, // placeholder
            wasi_app_id: Some(app_id.to_string()),
        }
    }
}

pub const AVAILABLE_TOOLS: &[ChatToolKind] = &[
    ChatToolKind::DuckDuckGoSearch,
    ChatToolKind::ReadTextFile,
    ChatToolKind::WriteTextFile,
];

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct VirtualFs {
    pub files: HashMap<String, String>,
}

impl VirtualFs {
    pub fn read_text_file(&self, path: &str) -> Result<String, String> {
        self.files
            .get(path)
            .cloned()
            .ok_or_else(|| format!("File not found: {}", path))
    }
    
    pub fn write_text_file(&mut self, path: &str, content: &str) -> Result<(), String> {
        self.files.insert(path.to_string(), content.to_string());
        Ok(())
    }
    
    pub fn list_files(&self) -> Vec<String> {
        self.files.keys().cloned().collect()
    }
}

pub type VfsHandle = Arc<Mutex<VirtualFs>>;

pub fn new_vfs() -> VfsHandle {
    Arc::new(Mutex::new(VirtualFs::default()))
}

pub fn vfs_from_json(json: &str) -> VfsHandle {
    let vfs: VirtualFs = serde_json::from_str(json).unwrap_or_default();
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
    tools.iter().any(|tool| tool.kind == kind)
}

pub fn has_wasi_tool(tools: &[ChatToolConfig]) -> bool {
    tools.iter().any(|tool| tool.wasi_app_id.is_some())
}

pub fn build_agent_preamble(system_prompt: &str, active_tools: &[ChatToolConfig]) -> String {
    let base = if system_prompt.trim().is_empty() {
        "You are a helpful assistant.".to_string()
    } else {
        system_prompt.trim().to_string()
    };

    if has_tool(active_tools, ChatToolKind::DuckDuckGoSearch) {
        format!(
            "{base}\n\nWhen the user needs fresh web information, local recommendations, live availability, recent events, or facts you are not confident about, call the DuckDuckGo Search tool. Never pretend you searched if you did not use the tool.\n\nAt the end of your response, if you used any tools, briefly list what searches you performed in the format: [Searches: query1, query2, ...]",
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

pub fn parse_tool_invocations(response: &str) -> (String, Vec<ToolInvocation>) {
    let search_patterns = [
        "[Searches:",
        "[Search:",
        "Searches:",
        "Search:",
    ];
    
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
                    .map(|s| s.trim().split(',').map(|q| q.trim().trim_matches('"').trim_matches('\'').to_string()).filter(|q| !q.is_empty()).collect::<Vec<_>>())
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
            description: "Read the contents of a text file from the virtual filesystem.".to_string(),
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
        let vfs = self.vfs.lock().map_err(|e| ReadTextFileError(e.to_string()))?;
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
        let mut vfs = self.vfs.lock().map_err(|e| WriteTextFileError(e.to_string()))?;
        vfs.write_text_file(&args.path, &args.content).map_err(WriteTextFileError)?;
        Ok(format!("Successfully wrote to {}", args.path))
    }
}

// ── WASI App Tool ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct WasiAppArgs {
    pub args: String,
}

#[derive(Debug, Error)]
#[error("{0}")]
pub struct WasiAppError(pub String);

#[derive(Clone)]
pub struct WasiAppTool {
    pub name: String,
    pub description: String,
    pub bytes: Vec<u8>,
    pub vfs: VfsHandle,
}

impl WasiAppTool {
    pub fn new(name: &str, description: &str, bytes: Vec<u8>, vfs: VfsHandle) -> Self {
        Self {
            name: name.to_string(),
            description: description.to_string(),
            bytes,
            vfs,
        }
    }

    pub fn id(&self) -> String {
        format!("wasi_{}", self.name.replace(' ', "_").to_lowercase())
    }
}

impl Tool for WasiAppTool {
    const NAME: &'static str = "wasi_app";

    type Error = WasiAppError;
    type Args = WasiAppArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: self.id(),
            description: self.description.clone(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "args": {
                        "type": "string",
                        "description": "Command line arguments to pass to the WASI application"
                    }
                },
                "required": ["args"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        run_wasi_cli(&self.bytes, &self.name, &args.args)
            .await
            .map_err(WasiAppError)
    }
}

async fn run_wasi_cli(wasm_bytes: &[u8], module_name: &str, cli_args: &str) -> Result<String, String> {
    let (stdout_tx, mut stdout_rx) = Pipe::channel();
    let (stderr_tx, mut stderr_rx) = Pipe::channel();

    let args_vec: Vec<String> = std::iter::once(module_name.to_string())
        .chain(cli_args.split_whitespace().map(String::from))
        .collect();

    let mut store = Store::default();
    let engine = store.engine().clone();
    let module = Module::new(&store, wasm_bytes)
        .map_err(|e| format!("Failed to compile wasm: {e}"))?;

    let mut wasi_env_builder = WasiEnv::builder(module_name)
        .args(&args_vec)
        .stdout(Box::new(stdout_tx))
        .stderr(Box::new(stderr_tx));
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
        start.call(&mut store, &[]).map_err(|e| format!("WASI execution error: {e}"))?;
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

pub async fn generate_help_text(wasm_bytes: &[u8], module_name: &str) -> String {
    for help_arg in &["-h", "--help", "help"] {
        let result = run_wasi_cli(wasm_bytes, module_name, help_arg).await;
        if let Ok(output) = result {
            if !output.contains("unrecognized") && !output.contains("unknown") && !output.contains("invalid") {
                return output;
            }
        }
    }
    "No help available".to_string()
}
