//! WASI stdio provider.
//!
//! Runs a `.wasm` module compiled to `wasm32-wasip1` (WASI preview 1).
//! The conversation history is passed to the module as:
//!   - program arguments: `["chat"]`
//!   - stdin: the user message, newline-terminated, then EOF
//!   - an env var `CHAT_HISTORY` containing the JSON-encoded prior messages
//!
//! The module's stdout is captured and streamed back token-by-token
//! (line-by-line) through a `tokio::sync::mpsc` channel so the UI can
//! display streaming output in real time.

use tokio::sync::mpsc;
use wasmer::{Instance, Module, Store};
use wasmer_wasix::{Pipe, WasiEnv, WasiFunctionEnv};

use super::{Message, ProviderError};
use crate::db;

/// Run a WASM/WASI module and stream its stdout back as chunks.
///
/// * `wasm_bytes` – raw `.wasm` binary (already loaded from DB).
/// * `module_name` – human label used as the WASI program name (`argv[0]`).
/// * `messages` – full conversation history; the last user message is written
///   to stdin followed by EOF, the rest is available in `CHAT_HISTORY`.
///
/// Returns a channel receiver. Each `Ok(String)` is a token/chunk to append
/// to the assistant bubble. The channel closes when the module exits.
pub fn chat_stream(
    module_path: String,
    module_name: String,
    messages: Vec<Message>,
    workspace_root: std::path::PathBuf,
) -> mpsc::Receiver<Result<String, ProviderError>> {
    let (tx, rx) = mpsc::channel(64);

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to create Tokio runtime");

        let result = rt.block_on(run_wasi(
            &module_path,
            &module_name,
            &messages,
            &workspace_root,
        ));
        match result {
            Ok(output) => {
                for line in output.lines() {
                    let chunk = format!("{}\n", line);
                    let _ = tx.blocking_send(Ok(chunk));
                }
            }
            Err(e) => {
                let _ = tx.blocking_send(Err(e));
            }
        }
    });

    rx
}

async fn run_wasi(
    module_path: &str,
    module_name: &str,
    messages: &[Message],
    _workspace_root: &std::path::PathBuf,
) -> Result<String, ProviderError> {
    // ── Prepare I/O pipes ─────────────────────────────────────────────────────
    let (stdout_tx, mut stdout_rx) = Pipe::channel();
    let (stderr_tx, mut stderr_rx) = Pipe::channel();

    // Stdin: pipe the last user message as stdin text so modules can also
    // use `std::io::stdin()` to read it.
    let user_input = messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| m.content.as_str())
        .unwrap_or("");
    let mut stdin_bytes = user_input.as_bytes().to_vec();
    if !stdin_bytes.ends_with(b"\n") {
        stdin_bytes.push(b'\n');
    }
    let (mut stdin_tx, stdin_rx) = Pipe::channel();
    use std::io::Write;
    stdin_tx
        .write_all(&stdin_bytes)
        .map_err(|e| ProviderError::Io(format!("stdin write: {e}")))?;
    drop(stdin_tx);

    // ── Build args & env ──────────────────────────────────────────────────────
    let args: Vec<String> = vec![module_name.to_string()];
    let history_json = serde_json::to_string(messages).unwrap_or_else(|_| "[]".to_string());

    // ── Compile ───────────────────────────────────────────────────────────────
    let mut store = Store::default();
    let engine = store.engine().clone();
    let wasm_bytes = db::read_storage_bytes(module_path)
        .map_err(|e| ProviderError::Io(format!("Failed to read wasm: {e}")))?;
    let module = Module::new(&store, wasm_bytes)
        .map_err(|e| ProviderError::Parse(format!("Failed to compile wasm: {e}")))?;

    // ── Create WASI environment ───────────────────────────────────────────────
    let mut wasi_env_builder = WasiEnv::builder(module_name)
        .args(&args)
        .env("CHAT_HISTORY", &history_json)
        .stdin(Box::new(stdin_rx))
        .stdout(Box::new(stdout_tx))
        .stderr(Box::new(stderr_tx));
    
    // Don't mount real directories - let WASI use its virtual filesystem
    wasi_env_builder.set_engine(engine);
    let wasi_env = wasi_env_builder
        .build()
        .map_err(|e| ProviderError::Io(format!("WASI env build error: {e}")))?;

    // ── Generate imports and instantiate ─────────────────────────────────────
    let mut func_env = WasiFunctionEnv::new(&mut store, wasi_env);
    let imports = func_env
        .import_object(&mut store, &module)
        .map_err(|e| ProviderError::Io(format!("Failed to build WASI imports: {e}")))?;

    let instance = Instance::new(&mut store, &module, &imports)
        .map_err(|e| ProviderError::Io(format!("Failed to instantiate wasm: {e}")))?;
    func_env
        .initialize(&mut store, instance.clone())
        .map_err(|e| ProviderError::Io(format!("Failed to initialize WASI env: {e}")))?;

    // ── Run the module ────────────────────────────────────────────────────────
    // Try to find and call the _start function, or fall back to a main export
    let start = instance
        .exports
        .get_function("_start")
        .or_else(|_| instance.exports.get_function("_start"));

    if let Ok(start) = start {
        start
            .call(&mut store, &[])
            .map_err(|e| ProviderError::Io(format!("WASI execution error: {e}")))?;
    }

    // ── Read captured output ─────────────────────────────────────────────────
    drop(instance);
    drop(func_env);
    drop(store);

    let mut output = String::new();
    tokio::io::AsyncReadExt::read_to_string(&mut stdout_rx, &mut output)
        .await
        .map_err(|e| ProviderError::Io(format!("stdout read: {e}")))?;

    // Append stderr as a dimmed note if non-empty
    let mut stderr_out = String::new();
    let _ = tokio::io::AsyncReadExt::read_to_string(&mut stderr_rx, &mut stderr_out).await;
    if !stderr_out.trim().is_empty() {
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str(&format!(
            "\n---\n*stderr:*\n```\n{}\n```",
            stderr_out.trim()
        ));
    }

    if output.is_empty() {
        output = "(no output)".to_string();
    }

    Ok(output)
}
