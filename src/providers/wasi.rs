//! WASI stdio provider.
//!
//! Runs a `.wasm` module compiled to `wasm32-wasip1` (WASI preview 1).
//! The conversation history is passed to the module as:
//!   - program arguments: `["chat", "<user_message>"]`
//!   - an env var `CHAT_HISTORY` containing the JSON-encoded prior messages
//!
//! The module's stdout is captured and streamed back token-by-token
//! (line-by-line) through a `tokio::sync::mpsc` channel so the UI can
//! display streaming output in real time.

use std::io::Read;

use tokio::sync::mpsc;
use wasmer::{FunctionEnv, Instance, Module, Store};
use wasmer_wasix::{generate_import_object_from_env, Pipe, WasiEnv, WasiVersion};

use super::{Message, ProviderError};

/// Run a WASM/WASI module and stream its stdout back as chunks.
///
/// * `wasm_bytes` – raw `.wasm` binary (already loaded from DB).
/// * `module_name` – human label used as the WASI program name (`argv[0]`).
/// * `messages` – full conversation history; the last user message is passed
///   as `argv[1]`, the rest as JSON in `CHAT_HISTORY`.
///
/// Returns a channel receiver. Each `Ok(String)` is a token/chunk to append
/// to the assistant bubble. The channel closes when the module exits.
pub fn chat_stream(
    wasm_bytes: Vec<u8>,
    module_name: String,
    messages: Vec<Message>,
) -> mpsc::Receiver<Result<String, ProviderError>> {
    let (tx, rx) = mpsc::channel(64);

    std::thread::spawn(move || {
        let result = run_wasi(&wasm_bytes, &module_name, &messages);
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

fn run_wasi(
    wasm_bytes: &[u8],
    module_name: &str,
    messages: &[Message],
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
    let stdin_bytes = user_input.as_bytes().to_vec();
    let (mut stdin_tx, stdin_rx) = Pipe::channel();
    use std::io::Write;
    stdin_tx
        .write_all(&stdin_bytes)
        .map_err(|e| ProviderError::Io(format!("stdin write: {e}")))?;
    drop(stdin_tx);

    // ── Build args & env ──────────────────────────────────────────────────────
    let args: Vec<String> = vec![module_name.to_string(), user_input.to_string()];
    let history_json = serde_json::to_string(messages).unwrap_or_else(|_| "[]".to_string());

    // ── Create WASI environment ───────────────────────────────────────────────
    let wasi_env = WasiEnv::builder(module_name)
        .args(&args)
        .env("CHAT_HISTORY", &history_json)
        .stdin(Box::new(stdin_rx))
        .stdout(Box::new(stdout_tx))
        .stderr(Box::new(stderr_tx))
        .build()
        .map_err(|e| ProviderError::Io(format!("WASI env build error: {e}")))?;

    // ── Compile ───────────────────────────────────────────────────────────────
    let mut store = Store::default();
    let module = Module::new(&store, wasm_bytes)
        .map_err(|e| ProviderError::Parse(format!("Failed to compile wasm: {e}")))?;

    // ── Generate imports and instantiate ─────────────────────────────────────
    let func_env = FunctionEnv::new(&mut store, wasi_env);
    let imports = generate_import_object_from_env(&mut store, &func_env, WasiVersion::Snapshot1);

    let instance = Instance::new(&mut store, &module, &imports)
        .map_err(|e| ProviderError::Io(format!("Failed to instantiate wasm: {e}")))?;

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
    let mut output = String::new();
    stdout_rx
        .read_to_string(&mut output)
        .map_err(|e| ProviderError::Io(format!("stdout read: {e}")))?;

    // Append stderr as a dimmed note if non-empty
    let mut stderr_out = String::new();
    stderr_rx.read_to_string(&mut stderr_out).ok();
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
