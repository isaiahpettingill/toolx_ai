//! Local SQLite persistence for chat history.

use chrono::Utc;
use rusqlite::{params, Connection, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatSummary {
    pub id: String,
    pub title: String,
    pub model: String,
    /// e.g. "builtin" | "ollama"
    pub provider: String,
    pub system_prompt: String,
    pub tools_json: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DbMessage {
    pub id: String,
    pub chat_id: String,
    pub role: String, // "user" | "assistant"
    pub content: String,
    pub created_at: String,
}

/// A stored WASM model binary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WasmModel {
    pub id: String,
    pub name: String,
    pub bytes: Vec<u8>,
    pub created_at: String,
}

fn db_path() -> PathBuf {
    let mut p = dirs_next();
    p.push("toolx_ai");
    std::fs::create_dir_all(&p).ok();
    p.push("chats.db");
    p
}

fn dirs_next() -> PathBuf {
    // Android: call Context.getFilesDir() via JNI to get the app's internal storage path.
    // ndk-context and jni are transitive deps on android (pulled in by dioxus-asset-resolver).
    #[cfg(target_os = "android")]
    {
        if let Some(path) = android_files_dir() {
            return path;
        }
        // Last-resort fallback
        return std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    }
    // Desktop / other Unix: ~/.local/share, Windows: %USERPROFILE%\.local\share
    #[cfg(not(target_os = "android"))]
    {
        if let Some(d) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
            let mut p = PathBuf::from(d);
            p.push(".local/share");
            p
        } else {
            PathBuf::from(".")
        }
    }
}

/// Call `android.content.Context.getFilesDir()` via JNI and return the path.
/// Returns `None` if any JNI call fails (e.g. called before the runtime is ready).
#[cfg(target_os = "android")]
fn android_files_dir() -> Option<PathBuf> {
    let ctx = ndk_context::android_context();
    let vm = unsafe { jni::JavaVM::from_raw(ctx.vm().cast()) }.ok()?;
    let mut env = vm.attach_current_thread().ok()?;

    let files_dir = env
        .call_method(
            unsafe { jni::objects::JObject::from_raw(ctx.context().cast()) },
            "getFilesDir",
            "()Ljava/io/File;",
            &[],
        )
        .ok()?
        .l()
        .ok()?;

    let path_str = env
        .call_method(&files_dir, "getAbsolutePath", "()Ljava/lang/String;", &[])
        .ok()?
        .l()
        .ok()?;

    let jstr = jni::objects::JString::from(path_str);
    let path: String = env.get_string(&jstr).ok()?.into();
    Some(PathBuf::from(path))
}

pub fn open() -> Result<Connection> {
    let conn = Connection::open(db_path())?;
    init_schema(conn)
}

/// Open an in-memory SQLite database. Used as a fallback when the on-disk path
/// is not accessible (e.g. first launch on Android before the files dir is ready).
pub fn open_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    init_schema(conn)
}

fn init_schema(conn: Connection) -> Result<Connection> {
    conn.execute_batch("PRAGMA journal_mode=WAL;")?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS chats (
            id            TEXT PRIMARY KEY,
            title         TEXT NOT NULL,
            model         TEXT NOT NULL DEFAULT 'echo:0b',
            provider      TEXT NOT NULL DEFAULT 'builtin',
            system_prompt TEXT NOT NULL DEFAULT '',
            tools_json    TEXT NOT NULL DEFAULT '[]',
            created_at    TEXT NOT NULL,
            updated_at    TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS messages (
            id          TEXT PRIMARY KEY,
            chat_id     TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
            role        TEXT NOT NULL,
            content     TEXT NOT NULL,
            created_at  TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS settings (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS wasm_models (
            id         TEXT PRIMARY KEY,
            name       TEXT NOT NULL,
            bytes      BLOB NOT NULL,
            created_at TEXT NOT NULL
        );",
    )?;
    // Migrations for existing DBs
    conn.execute_batch("ALTER TABLE chats ADD COLUMN provider TEXT NOT NULL DEFAULT 'builtin';")
        .ok();
    conn.execute_batch("ALTER TABLE chats ADD COLUMN system_prompt TEXT NOT NULL DEFAULT '';")
        .ok();
    conn.execute_batch("ALTER TABLE chats ADD COLUMN tools_json TEXT NOT NULL DEFAULT '[]';")
        .ok();
    Ok(conn)
}

/// Read a setting value by key, returning `None` if not set.
pub fn get_setting(conn: &Connection, key: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT value FROM settings WHERE key=?1")?;
    let mut rows = stmt.query(params![key])?;
    if let Some(row) = rows.next()? {
        Ok(Some(row.get(0)?))
    } else {
        Ok(None)
    }
}

/// Upsert a setting value.
pub fn set_setting(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub fn list_chats(conn: &Connection) -> Result<Vec<ChatSummary>> {
    let mut stmt = conn.prepare(
        "SELECT id, title, model, provider, system_prompt, tools_json, created_at, updated_at FROM chats ORDER BY updated_at DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(ChatSummary {
            id: row.get(0)?,
            title: row.get(1)?,
            model: row.get(2)?,
            provider: row.get(3)?,
            system_prompt: row.get(4)?,
            tools_json: row.get(5)?,
            created_at: row.get(6)?,
            updated_at: row.get(7)?,
        })
    })?;
    rows.collect()
}

pub fn create_chat(
    conn: &Connection,
    title: &str,
    model: &str,
    provider: &str,
) -> Result<ChatSummary> {
    let now = Utc::now().to_rfc3339();
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO chats (id, title, model, provider, system_prompt, tools_json, created_at, updated_at) VALUES (?1,?2,?3,?4,'','[]',?5,?6)",
        params![id, title, model, provider, now, now],
    )?;
    Ok(ChatSummary {
        id,
        title: title.to_string(),
        model: model.to_string(),
        provider: provider.to_string(),
        system_prompt: String::new(),
        tools_json: "[]".to_string(),
        created_at: now.clone(),
        updated_at: now,
    })
}

pub fn rename_chat(conn: &Connection, id: &str, title: &str) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE chats SET title=?1, updated_at=?2 WHERE id=?3",
        params![title, now, id],
    )?;
    Ok(())
}

pub fn delete_chat(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("DELETE FROM messages WHERE chat_id=?1", params![id])?;
    conn.execute("DELETE FROM chats WHERE id=?1", params![id])?;
    Ok(())
}

pub fn get_messages(conn: &Connection, chat_id: &str) -> Result<Vec<DbMessage>> {
    let mut stmt = conn.prepare(
        "SELECT id, chat_id, role, content, created_at FROM messages WHERE chat_id=?1 ORDER BY created_at ASC",
    )?;
    let rows = stmt.query_map(params![chat_id], |row| {
        Ok(DbMessage {
            id: row.get(0)?,
            chat_id: row.get(1)?,
            role: row.get(2)?,
            content: row.get(3)?,
            created_at: row.get(4)?,
        })
    })?;
    rows.collect()
}

pub fn add_message(
    conn: &Connection,
    chat_id: &str,
    role: &str,
    content: &str,
) -> Result<DbMessage> {
    let now = Utc::now().to_rfc3339();
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO messages (id, chat_id, role, content, created_at) VALUES (?1,?2,?3,?4,?5)",
        params![id, chat_id, role, content, now],
    )?;
    // bump chat updated_at
    conn.execute(
        "UPDATE chats SET updated_at=?1 WHERE id=?2",
        params![now, chat_id],
    )?;
    Ok(DbMessage {
        id,
        chat_id: chat_id.to_string(),
        role: role.to_string(),
        content: content.to_string(),
        created_at: now,
    })
}

pub fn update_chat_model(conn: &Connection, chat_id: &str, model: &str) -> Result<()> {
    conn.execute(
        "UPDATE chats SET model=?1 WHERE id=?2",
        params![model, chat_id],
    )?;
    Ok(())
}

pub fn update_chat_provider(conn: &Connection, chat_id: &str, provider: &str) -> Result<()> {
    conn.execute(
        "UPDATE chats SET provider=?1 WHERE id=?2",
        params![provider, chat_id],
    )?;
    Ok(())
}

pub fn update_chat_system_prompt(conn: &Connection, chat_id: &str, prompt: &str) -> Result<()> {
    conn.execute(
        "UPDATE chats SET system_prompt=?1 WHERE id=?2",
        params![prompt, chat_id],
    )?;
    Ok(())
}

pub fn update_chat_tools(conn: &Connection, chat_id: &str, tools_json: &str) -> Result<()> {
    conn.execute(
        "UPDATE chats SET tools_json=?1 WHERE id=?2",
        params![tools_json, chat_id],
    )?;
    Ok(())
}

// ── WASM model CRUD ───────────────────────────────────────────────────────────

pub fn list_wasm_models(conn: &Connection) -> Result<Vec<WasmModel>> {
    let mut stmt = conn
        .prepare("SELECT id, name, bytes, created_at FROM wasm_models ORDER BY created_at ASC")?;
    let rows = stmt.query_map([], |row| {
        Ok(WasmModel {
            id: row.get(0)?,
            name: row.get(1)?,
            bytes: row.get(2)?,
            created_at: row.get(3)?,
        })
    })?;
    rows.collect()
}

pub fn add_wasm_model(conn: &Connection, name: &str, bytes: &[u8]) -> Result<WasmModel> {
    let now = Utc::now().to_rfc3339();
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO wasm_models (id, name, bytes, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![id, name, bytes, now],
    )?;
    Ok(WasmModel {
        id,
        name: name.to_string(),
        bytes: bytes.to_vec(),
        created_at: now,
    })
}

pub fn get_wasm_model(conn: &Connection, id: &str) -> Result<Option<WasmModel>> {
    let mut stmt =
        conn.prepare("SELECT id, name, bytes, created_at FROM wasm_models WHERE id=?1")?;
    let mut rows = stmt.query(params![id])?;
    if let Some(row) = rows.next()? {
        Ok(Some(WasmModel {
            id: row.get(0)?,
            name: row.get(1)?,
            bytes: row.get(2)?,
            created_at: row.get(3)?,
        }))
    } else {
        Ok(None)
    }
}

pub fn delete_wasm_model(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("DELETE FROM wasm_models WHERE id=?1", params![id])?;
    Ok(())
}
