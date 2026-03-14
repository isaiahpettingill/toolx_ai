//! Local SQLite persistence and file-backed storage.

use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

fn io_to_sql(err: std::io::Error) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(err))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatSummary {
    pub id: String,
    pub title: String,
    pub model: String,
    pub provider: String,
    pub system_prompt: String,
    pub tools_json: String,
    pub vfs_json: String,
    pub embedding_model: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DbMessage {
    pub id: String,
    pub chat_id: String,
    pub role: String,
    pub content: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WasmModel {
    pub id: String,
    pub name: String,
    pub file_path: String,
    pub file_size: u64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WasiApp {
    pub id: String,
    pub name: String,
    pub description: String,
    pub help_text: String,
    pub file_path: String,
    pub file_size: u64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatFile {
    pub id: String,
    pub chat_id: String,
    pub path: String,
    pub display_name: String,
    pub file_path: String,
    pub mime_type: String,
    pub byte_size: u64,
    pub is_text: bool,
    pub inline_context: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KnowledgeBase {
    pub id: String,
    pub name: String,
    pub description: String,
    pub embedding_model: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KnowledgeBaseFile {
    pub id: String,
    pub knowledge_base_id: String,
    pub path: String,
    pub display_name: String,
    pub file_path: String,
    pub mime_type: String,
    pub byte_size: u64,
    pub is_text: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RagChunk {
    pub id: String,
    pub chat_id: Option<String>,
    pub knowledge_base_id: Option<String>,
    pub file_id: String,
    pub file_name: String,
    pub path: String,
    pub chunk_index: i64,
    pub content: String,
    pub term_freq_json: String,
    pub term_count: i64,
    pub embedding_model: String,
    pub embedding_json: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VirtualFs {
    pub files: HashMap<String, String>,
}

fn dirs_next() -> PathBuf {
    #[cfg(target_os = "android")]
    {
        if let Some(path) = android_files_dir() {
            return path;
        }
        return std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    }
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

pub fn app_dir() -> PathBuf {
    let mut p = dirs_next();
    p.push("toolx_ai");
    fs::create_dir_all(&p).ok();
    p
}

fn db_path() -> PathBuf {
    let mut p = app_dir();
    p.push("chats.db");
    p
}

pub fn storage_root() -> PathBuf {
    let mut p = app_dir();
    p.push("storage");
    fs::create_dir_all(&p).ok();
    p
}

pub fn chat_vfs_root(chat_id: &str) -> PathBuf {
    let mut p = storage_root();
    p.push("chat_vfs");
    p.push(chat_id);
    fs::create_dir_all(&p).ok();
    p
}

fn chat_upload_root(chat_id: &str) -> PathBuf {
    let mut p = storage_root();
    p.push("chat_uploads");
    p.push(chat_id);
    fs::create_dir_all(&p).ok();
    p
}

fn knowledge_base_root(knowledge_base_id: &str) -> PathBuf {
    let mut p = storage_root();
    p.push("knowledge_bases");
    p.push(knowledge_base_id);
    fs::create_dir_all(&p).ok();
    p
}

fn artifact_root(kind: &str) -> PathBuf {
    let mut p = storage_root();
    p.push(kind);
    fs::create_dir_all(&p).ok();
    p
}

fn safe_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    let trimmed = out.trim_matches('_');
    if trimmed.is_empty() {
        "file".to_string()
    } else {
        trimmed.to_string()
    }
}

fn storage_relative(path: &Path) -> String {
    let root = storage_root();
    path.strip_prefix(&root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

pub fn resolve_storage_path(relative: &str) -> PathBuf {
    let mut p = storage_root();
    for part in relative.split('/').filter(|part| !part.is_empty()) {
        p.push(part);
    }
    p
}

fn write_named_file(
    parent: &Path,
    id: &str,
    name: &str,
    bytes: &[u8],
) -> std::io::Result<(String, u64)> {
    fs::create_dir_all(parent)?;
    let filename = format!("{}-{}", id, safe_name(name));
    let path = parent.join(filename);
    fs::write(&path, bytes)?;
    Ok((storage_relative(&path), bytes.len() as u64))
}

fn remove_storage_file(relative: &str) {
    if relative.is_empty() {
        return;
    }
    let path = resolve_storage_path(relative);
    let _ = fs::remove_file(path);
}

pub fn read_storage_bytes(relative: &str) -> std::io::Result<Vec<u8>> {
    fs::read(resolve_storage_path(relative))
}

pub fn read_storage_text(relative: &str) -> std::io::Result<String> {
    fs::read_to_string(resolve_storage_path(relative))
}

pub fn open() -> Result<Connection> {
    let conn = Connection::open(db_path())?;
    init_schema(conn)
}

pub fn open_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    init_schema(conn)
}

fn init_schema(conn: Connection) -> Result<Connection> {
    conn.execute_batch("PRAGMA journal_mode=WAL;")?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS chats (
            id              TEXT PRIMARY KEY,
            title           TEXT NOT NULL,
            model           TEXT NOT NULL DEFAULT 'echo:0b',
            provider        TEXT NOT NULL DEFAULT 'builtin',
            system_prompt   TEXT NOT NULL DEFAULT '',
            tools_json      TEXT NOT NULL DEFAULT '[]',
            vfs_json        TEXT NOT NULL DEFAULT '{}',
            embedding_model TEXT NOT NULL DEFAULT '',
            created_at      TEXT NOT NULL,
            updated_at      TEXT NOT NULL
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
            file_path  TEXT NOT NULL DEFAULT '',
            file_size  INTEGER NOT NULL DEFAULT 0,
            bytes      BLOB,
            created_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS wasi_apps (
            id           TEXT PRIMARY KEY,
            name         TEXT NOT NULL,
            description  TEXT NOT NULL DEFAULT '',
            help_text    TEXT NOT NULL DEFAULT '',
            file_path    TEXT NOT NULL DEFAULT '',
            file_size    INTEGER NOT NULL DEFAULT 0,
            bytes        BLOB,
            created_at   TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS chat_files (
            id            TEXT PRIMARY KEY,
            chat_id       TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
            path          TEXT NOT NULL,
            display_name  TEXT NOT NULL,
            file_path     TEXT NOT NULL,
            mime_type     TEXT NOT NULL DEFAULT '',
            byte_size     INTEGER NOT NULL DEFAULT 0,
            is_text       INTEGER NOT NULL DEFAULT 0,
            inline_context TEXT NOT NULL DEFAULT '',
            created_at    TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS knowledge_bases (
            id              TEXT PRIMARY KEY,
            name            TEXT NOT NULL,
            description     TEXT NOT NULL DEFAULT '',
            embedding_model TEXT NOT NULL DEFAULT '',
            created_at      TEXT NOT NULL,
            updated_at      TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS knowledge_base_files (
            id                TEXT PRIMARY KEY,
            knowledge_base_id TEXT NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
            path              TEXT NOT NULL,
            display_name      TEXT NOT NULL,
            file_path         TEXT NOT NULL,
            mime_type         TEXT NOT NULL DEFAULT '',
            byte_size         INTEGER NOT NULL DEFAULT 0,
            is_text           INTEGER NOT NULL DEFAULT 0,
            created_at        TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS chat_knowledge_bases (
            chat_id            TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
            knowledge_base_id  TEXT NOT NULL REFERENCES knowledge_bases(id) ON DELETE CASCADE,
            created_at         TEXT NOT NULL,
            PRIMARY KEY (chat_id, knowledge_base_id)
        );
        CREATE TABLE IF NOT EXISTS rag_chunks (
            id                TEXT PRIMARY KEY,
            chat_id           TEXT,
            knowledge_base_id TEXT,
            file_id           TEXT NOT NULL,
            file_name         TEXT NOT NULL,
            path              TEXT NOT NULL,
            chunk_index       INTEGER NOT NULL,
            content           TEXT NOT NULL,
            term_freq_json    TEXT NOT NULL,
            term_count        INTEGER NOT NULL,
            embedding_model   TEXT NOT NULL DEFAULT '',
            embedding_json    TEXT NOT NULL DEFAULT '[]',
            created_at        TEXT NOT NULL
        );",
    )?;

    conn.execute_batch("ALTER TABLE chats ADD COLUMN vfs_json TEXT NOT NULL DEFAULT '{}';")
        .ok();
    conn.execute_batch("ALTER TABLE chats ADD COLUMN embedding_model TEXT NOT NULL DEFAULT ''; ")
        .ok();
    conn.execute_batch("ALTER TABLE wasm_models ADD COLUMN file_path TEXT NOT NULL DEFAULT ''; ")
        .ok();
    conn.execute_batch("ALTER TABLE wasm_models ADD COLUMN file_size INTEGER NOT NULL DEFAULT 0; ")
        .ok();
    conn.execute_batch("ALTER TABLE wasi_apps ADD COLUMN file_path TEXT NOT NULL DEFAULT ''; ")
        .ok();
    conn.execute_batch("ALTER TABLE wasi_apps ADD COLUMN file_size INTEGER NOT NULL DEFAULT 0; ")
        .ok();

    Ok(conn)
}

pub fn get_setting(conn: &Connection, key: &str) -> Result<Option<String>> {
    conn.query_row(
        "SELECT value FROM settings WHERE key=?1",
        params![key],
        |row| row.get(0),
    )
    .optional()
}

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
        "SELECT id, title, model, provider, system_prompt, tools_json, vfs_json, embedding_model, created_at, updated_at FROM chats ORDER BY updated_at DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(ChatSummary {
            id: row.get(0)?,
            title: row.get(1)?,
            model: row.get(2)?,
            provider: row.get(3)?,
            system_prompt: row.get(4)?,
            tools_json: row.get(5)?,
            vfs_json: row.get(6)?,
            embedding_model: row.get(7)?,
            created_at: row.get(8)?,
            updated_at: row.get(9)?,
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
        "INSERT INTO chats (id, title, model, provider, system_prompt, tools_json, vfs_json, embedding_model, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, '', '[]', '{}', '', ?5, ?6)",
        params![id, title, model, provider, now, now],
    )?;
    Ok(ChatSummary {
        id,
        title: title.to_string(),
        model: model.to_string(),
        provider: provider.to_string(),
        system_prompt: String::new(),
        tools_json: "[]".to_string(),
        vfs_json: "{}".to_string(),
        embedding_model: String::new(),
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
    for file in list_chat_files(conn, id).unwrap_or_default() {
        remove_storage_file(&file.file_path);
    }
    let chat_root = chat_vfs_root(id);
    let _ = fs::remove_dir_all(chat_root);
    let upload_root = chat_upload_root(id);
    let _ = fs::remove_dir_all(upload_root);
    conn.execute("DELETE FROM rag_chunks WHERE chat_id=?1", params![id])?;
    conn.execute(
        "DELETE FROM chat_knowledge_bases WHERE chat_id=?1",
        params![id],
    )?;
    conn.execute("DELETE FROM chat_files WHERE chat_id=?1", params![id])?;
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
        "INSERT INTO messages (id, chat_id, role, content, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, chat_id, role, content, now],
    )?;
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

pub fn update_chat_embedding_model(
    conn: &Connection,
    chat_id: &str,
    embedding_model: &str,
) -> Result<()> {
    conn.execute(
        "UPDATE chats SET embedding_model=?1 WHERE id=?2",
        params![embedding_model, chat_id],
    )?;
    Ok(())
}

fn maybe_materialize_legacy_wasm(
    conn: &Connection,
    id: &str,
    name: &str,
    file_path: &str,
    file_size: u64,
    bytes: Option<Vec<u8>>,
) -> Result<(String, u64)> {
    if !file_path.is_empty() {
        return Ok((file_path.to_string(), file_size));
    }
    let Some(bytes) = bytes else {
        return Ok((String::new(), file_size));
    };
    let (new_path, new_size) =
        write_named_file(&artifact_root("wasm_models"), id, name, &bytes).map_err(io_to_sql)?;
    conn.execute(
        "UPDATE wasm_models SET file_path=?1, file_size=?2, bytes=NULL WHERE id=?3",
        params![new_path, new_size as i64, id],
    )?;
    Ok((new_path, new_size))
}

fn maybe_materialize_legacy_wasi_app(
    conn: &Connection,
    id: &str,
    name: &str,
    file_path: &str,
    file_size: u64,
    bytes: Option<Vec<u8>>,
) -> Result<(String, u64)> {
    if !file_path.is_empty() {
        return Ok((file_path.to_string(), file_size));
    }
    let Some(bytes) = bytes else {
        return Ok((String::new(), file_size));
    };
    let (new_path, new_size) =
        write_named_file(&artifact_root("wasi_apps"), id, name, &bytes).map_err(io_to_sql)?;
    conn.execute(
        "UPDATE wasi_apps SET file_path=?1, file_size=?2, bytes=NULL WHERE id=?3",
        params![new_path, new_size as i64, id],
    )?;
    Ok((new_path, new_size))
}

pub fn list_wasm_models(conn: &Connection) -> Result<Vec<WasmModel>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, file_path, file_size, bytes, created_at FROM wasm_models ORDER BY created_at ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        let id: String = row.get(0)?;
        let name: String = row.get(1)?;
        let file_path: String = row.get(2)?;
        let file_size = row.get::<_, i64>(3)? as u64;
        let bytes: Option<Vec<u8>> = row.get(4)?;
        let created_at: String = row.get(5)?;
        let (file_path, file_size) =
            maybe_materialize_legacy_wasm(conn, &id, &name, &file_path, file_size, bytes)?;
        Ok(WasmModel {
            id,
            name,
            file_path,
            file_size,
            created_at,
        })
    })?;
    rows.collect()
}

pub fn add_wasm_model(conn: &Connection, name: &str, bytes: &[u8]) -> Result<WasmModel> {
    let now = Utc::now().to_rfc3339();
    let id = Uuid::new_v4().to_string();
    let (file_path, file_size) =
        write_named_file(&artifact_root("wasm_models"), &id, name, bytes).map_err(io_to_sql)?;
    conn.execute(
        "INSERT INTO wasm_models (id, name, file_path, file_size, bytes, created_at) VALUES (?1, ?2, ?3, ?4, NULL, ?5)",
        params![id, name, file_path, file_size as i64, now],
    )?;
    Ok(WasmModel {
        id,
        name: name.to_string(),
        file_path,
        file_size,
        created_at: now,
    })
}

pub fn get_wasm_model(conn: &Connection, id: &str) -> Result<Option<WasmModel>> {
    conn.query_row(
        "SELECT id, name, file_path, file_size, bytes, created_at FROM wasm_models WHERE id=?1",
        params![id],
        |row| {
            let id: String = row.get(0)?;
            let name: String = row.get(1)?;
            let file_path: String = row.get(2)?;
            let file_size = row.get::<_, i64>(3)? as u64;
            let bytes: Option<Vec<u8>> = row.get(4)?;
            let created_at: String = row.get(5)?;
            let (file_path, file_size) =
                maybe_materialize_legacy_wasm(conn, &id, &name, &file_path, file_size, bytes)?;
            Ok(WasmModel {
                id,
                name,
                file_path,
                file_size,
                created_at,
            })
        },
    )
    .optional()
}

pub fn delete_wasm_model(conn: &Connection, id: &str) -> Result<()> {
    if let Some(model) = get_wasm_model(conn, id)? {
        remove_storage_file(&model.file_path);
    }
    conn.execute("DELETE FROM wasm_models WHERE id=?1", params![id])?;
    Ok(())
}

pub fn list_wasi_apps(conn: &Connection) -> Result<Vec<WasiApp>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, description, help_text, file_path, file_size, bytes, created_at FROM wasi_apps ORDER BY created_at ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        let id: String = row.get(0)?;
        let name: String = row.get(1)?;
        let description: String = row.get(2)?;
        let help_text: String = row.get(3)?;
        let file_path: String = row.get(4)?;
        let file_size = row.get::<_, i64>(5)? as u64;
        let bytes: Option<Vec<u8>> = row.get(6)?;
        let created_at: String = row.get(7)?;
        let (file_path, file_size) =
            maybe_materialize_legacy_wasi_app(conn, &id, &name, &file_path, file_size, bytes)?;
        Ok(WasiApp {
            id,
            name,
            description,
            help_text,
            file_path,
            file_size,
            created_at,
        })
    })?;
    rows.collect()
}

pub fn add_wasi_app(
    conn: &Connection,
    name: &str,
    description: &str,
    help_text: &str,
    bytes: &[u8],
) -> Result<WasiApp> {
    let now = Utc::now().to_rfc3339();
    let id = Uuid::new_v4().to_string();
    let (file_path, file_size) =
        write_named_file(&artifact_root("wasi_apps"), &id, name, bytes).map_err(io_to_sql)?;
    conn.execute(
        "INSERT INTO wasi_apps (id, name, description, help_text, file_path, file_size, bytes, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7)",
        params![id, name, description, help_text, file_path, file_size as i64, now],
    )?;
    Ok(WasiApp {
        id,
        name: name.to_string(),
        description: description.to_string(),
        help_text: help_text.to_string(),
        file_path,
        file_size,
        created_at: now,
    })
}

pub fn get_wasi_app(conn: &Connection, id: &str) -> Result<Option<WasiApp>> {
    conn.query_row(
        "SELECT id, name, description, help_text, file_path, file_size, bytes, created_at FROM wasi_apps WHERE id=?1",
        params![id],
        |row| {
            let id: String = row.get(0)?;
            let name: String = row.get(1)?;
            let description: String = row.get(2)?;
            let help_text: String = row.get(3)?;
            let file_path: String = row.get(4)?;
            let file_size = row.get::<_, i64>(5)? as u64;
            let bytes: Option<Vec<u8>> = row.get(6)?;
            let created_at: String = row.get(7)?;
            let (file_path, file_size) = maybe_materialize_legacy_wasi_app(conn, &id, &name, &file_path, file_size, bytes)?;
            Ok(WasiApp { id, name, description, help_text, file_path, file_size, created_at })
        },
    )
    .optional()
}

pub fn update_wasi_app(conn: &Connection, id: &str, description: &str) -> Result<()> {
    conn.execute(
        "UPDATE wasi_apps SET description=?1 WHERE id=?2",
        params![description, id],
    )?;
    Ok(())
}

pub fn delete_wasi_app(conn: &Connection, id: &str) -> Result<()> {
    if let Some(app) = get_wasi_app(conn, id)? {
        remove_storage_file(&app.file_path);
    }
    conn.execute("DELETE FROM wasi_apps WHERE id=?1", params![id])?;
    Ok(())
}

pub fn get_chat_vfs(conn: &Connection, chat_id: &str) -> Result<VirtualFs> {
    let vfs_json: Option<String> = conn
        .query_row(
            "SELECT vfs_json FROM chats WHERE id=?1",
            params![chat_id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(vfs_json
        .and_then(|value| serde_json::from_str(&value).ok())
        .unwrap_or_default())
}

pub fn update_chat_vfs(conn: &Connection, chat_id: &str, vfs: &VirtualFs) -> Result<()> {
    let vfs_json = serde_json::to_string(vfs).unwrap_or_else(|_| "{}".to_string());
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE chats SET vfs_json=?1, updated_at=?2 WHERE id=?3",
        params![vfs_json, now, chat_id],
    )?;
    Ok(())
}

pub fn upsert_chat_vfs_text_file(chat_id: &str, path: &str, content: &str) -> std::io::Result<()> {
    let full_path = chat_vfs_root(chat_id).join(path.trim_start_matches('/'));
    if let Some(parent) = full_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(full_path, content)
}

pub fn list_chat_vfs_paths(chat_id: &str) -> Vec<String> {
    let root = chat_vfs_root(chat_id);
    let mut out = Vec::new();
    collect_relative_files(&root, &root, &mut out);
    out.sort();
    out
}

fn collect_relative_files(root: &Path, current: &Path, out: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(current) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_relative_files(root, &path, out);
        } else if let Ok(relative) = path.strip_prefix(root) {
            out.push(format!(
                "/{}",
                relative.to_string_lossy().replace('\\', "/")
            ));
        }
    }
}

pub fn add_chat_file(
    conn: &Connection,
    chat_id: &str,
    path: &str,
    display_name: &str,
    mime_type: &str,
    bytes: &[u8],
    is_text: bool,
    inline_context: &str,
) -> Result<ChatFile> {
    let now = Utc::now().to_rfc3339();
    let id = Uuid::new_v4().to_string();
    let (file_path, byte_size) =
        write_named_file(&chat_upload_root(chat_id), &id, display_name, bytes)
            .map_err(io_to_sql)?;

    let vfs_path = chat_vfs_root(chat_id).join(path.trim_start_matches('/'));
    if let Some(parent) = vfs_path.parent() {
        fs::create_dir_all(parent).ok();
    }
    fs::write(&vfs_path, bytes).map_err(io_to_sql)?;

    conn.execute(
        "INSERT INTO chat_files (id, chat_id, path, display_name, file_path, mime_type, byte_size, is_text, inline_context, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            id,
            chat_id,
            path,
            display_name,
            file_path,
            mime_type,
            byte_size as i64,
            if is_text { 1 } else { 0 },
            inline_context,
            now
        ],
    )?;
    Ok(ChatFile {
        id,
        chat_id: chat_id.to_string(),
        path: path.to_string(),
        display_name: display_name.to_string(),
        file_path,
        mime_type: mime_type.to_string(),
        byte_size,
        is_text,
        inline_context: inline_context.to_string(),
        created_at: now,
    })
}

pub fn list_chat_files(conn: &Connection, chat_id: &str) -> Result<Vec<ChatFile>> {
    let mut stmt = conn.prepare(
        "SELECT id, chat_id, path, display_name, file_path, mime_type, byte_size, is_text, inline_context, created_at
         FROM chat_files WHERE chat_id=?1 ORDER BY created_at ASC",
    )?;
    let rows = stmt.query_map(params![chat_id], |row| {
        Ok(ChatFile {
            id: row.get(0)?,
            chat_id: row.get(1)?,
            path: row.get(2)?,
            display_name: row.get(3)?,
            file_path: row.get(4)?,
            mime_type: row.get(5)?,
            byte_size: row.get::<_, i64>(6)? as u64,
            is_text: row.get::<_, i64>(7)? != 0,
            inline_context: row.get(8)?,
            created_at: row.get(9)?,
        })
    })?;
    rows.collect()
}

pub fn list_chat_inline_contexts(conn: &Connection, chat_id: &str) -> Result<Vec<ChatFile>> {
    Ok(list_chat_files(conn, chat_id)?
        .into_iter()
        .filter(|file| !file.inline_context.is_empty())
        .collect())
}

pub fn create_knowledge_base(
    conn: &Connection,
    name: &str,
    description: &str,
    embedding_model: &str,
) -> Result<KnowledgeBase> {
    let now = Utc::now().to_rfc3339();
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO knowledge_bases (id, name, description, embedding_model, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, name, description, embedding_model, now, now],
    )?;
    Ok(KnowledgeBase {
        id,
        name: name.to_string(),
        description: description.to_string(),
        embedding_model: embedding_model.to_string(),
        created_at: now.clone(),
        updated_at: now,
    })
}

pub fn list_knowledge_bases(conn: &Connection) -> Result<Vec<KnowledgeBase>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, description, embedding_model, created_at, updated_at FROM knowledge_bases ORDER BY updated_at DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(KnowledgeBase {
            id: row.get(0)?,
            name: row.get(1)?,
            description: row.get(2)?,
            embedding_model: row.get(3)?,
            created_at: row.get(4)?,
            updated_at: row.get(5)?,
        })
    })?;
    rows.collect()
}

pub fn delete_knowledge_base(conn: &Connection, knowledge_base_id: &str) -> Result<()> {
    for file in list_knowledge_base_files(conn, knowledge_base_id).unwrap_or_default() {
        remove_storage_file(&file.file_path);
    }
    let _ = fs::remove_dir_all(knowledge_base_root(knowledge_base_id));
    conn.execute(
        "DELETE FROM rag_chunks WHERE knowledge_base_id=?1",
        params![knowledge_base_id],
    )?;
    conn.execute(
        "DELETE FROM knowledge_base_files WHERE knowledge_base_id=?1",
        params![knowledge_base_id],
    )?;
    conn.execute(
        "DELETE FROM chat_knowledge_bases WHERE knowledge_base_id=?1",
        params![knowledge_base_id],
    )?;
    conn.execute(
        "DELETE FROM knowledge_bases WHERE id=?1",
        params![knowledge_base_id],
    )?;
    Ok(())
}

pub fn add_knowledge_base_file(
    conn: &Connection,
    knowledge_base_id: &str,
    path: &str,
    display_name: &str,
    mime_type: &str,
    bytes: &[u8],
    is_text: bool,
) -> Result<KnowledgeBaseFile> {
    let now = Utc::now().to_rfc3339();
    let id = Uuid::new_v4().to_string();
    let (file_path, byte_size) = write_named_file(
        &knowledge_base_root(knowledge_base_id),
        &id,
        display_name,
        bytes,
    )
    .map_err(io_to_sql)?;
    conn.execute(
        "INSERT INTO knowledge_base_files (id, knowledge_base_id, path, display_name, file_path, mime_type, byte_size, is_text, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            id,
            knowledge_base_id,
            path,
            display_name,
            file_path,
            mime_type,
            byte_size as i64,
            if is_text { 1 } else { 0 },
            now
        ],
    )?;
    Ok(KnowledgeBaseFile {
        id,
        knowledge_base_id: knowledge_base_id.to_string(),
        path: path.to_string(),
        display_name: display_name.to_string(),
        file_path,
        mime_type: mime_type.to_string(),
        byte_size,
        is_text,
        created_at: now,
    })
}

pub fn list_knowledge_base_files(
    conn: &Connection,
    knowledge_base_id: &str,
) -> Result<Vec<KnowledgeBaseFile>> {
    let mut stmt = conn.prepare(
        "SELECT id, knowledge_base_id, path, display_name, file_path, mime_type, byte_size, is_text, created_at
         FROM knowledge_base_files WHERE knowledge_base_id=?1 ORDER BY created_at ASC",
    )?;
    let rows = stmt.query_map(params![knowledge_base_id], |row| {
        Ok(KnowledgeBaseFile {
            id: row.get(0)?,
            knowledge_base_id: row.get(1)?,
            path: row.get(2)?,
            display_name: row.get(3)?,
            file_path: row.get(4)?,
            mime_type: row.get(5)?,
            byte_size: row.get::<_, i64>(6)? as u64,
            is_text: row.get::<_, i64>(7)? != 0,
            created_at: row.get(8)?,
        })
    })?;
    rows.collect()
}

pub fn attach_knowledge_base_to_chat(
    conn: &Connection,
    chat_id: &str,
    knowledge_base_id: &str,
) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT OR IGNORE INTO chat_knowledge_bases (chat_id, knowledge_base_id, created_at) VALUES (?1, ?2, ?3)",
        params![chat_id, knowledge_base_id, now],
    )?;
    Ok(())
}

pub fn detach_knowledge_base_from_chat(
    conn: &Connection,
    chat_id: &str,
    knowledge_base_id: &str,
) -> Result<()> {
    conn.execute(
        "DELETE FROM chat_knowledge_bases WHERE chat_id=?1 AND knowledge_base_id=?2",
        params![chat_id, knowledge_base_id],
    )?;
    Ok(())
}

pub fn list_chat_knowledge_bases(conn: &Connection, chat_id: &str) -> Result<Vec<KnowledgeBase>> {
    let mut stmt = conn.prepare(
        "SELECT kb.id, kb.name, kb.description, kb.embedding_model, kb.created_at, kb.updated_at
         FROM knowledge_bases kb
         INNER JOIN chat_knowledge_bases ckb ON ckb.knowledge_base_id = kb.id
         WHERE ckb.chat_id=?1
         ORDER BY kb.updated_at DESC",
    )?;
    let rows = stmt.query_map(params![chat_id], |row| {
        Ok(KnowledgeBase {
            id: row.get(0)?,
            name: row.get(1)?,
            description: row.get(2)?,
            embedding_model: row.get(3)?,
            created_at: row.get(4)?,
            updated_at: row.get(5)?,
        })
    })?;
    rows.collect()
}

pub fn list_chat_knowledge_base_ids(conn: &Connection, chat_id: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT knowledge_base_id FROM chat_knowledge_bases WHERE chat_id=?1 ORDER BY created_at ASC",
    )?;
    let rows = stmt.query_map(params![chat_id], |row| row.get(0))?;
    rows.collect()
}

pub fn clear_rag_chunks_for_file(conn: &Connection, file_id: &str) -> Result<()> {
    conn.execute("DELETE FROM rag_chunks WHERE file_id=?1", params![file_id])?;
    Ok(())
}

pub fn add_rag_chunk(
    conn: &Connection,
    chat_id: Option<&str>,
    knowledge_base_id: Option<&str>,
    file_id: &str,
    file_name: &str,
    path: &str,
    chunk_index: i64,
    content: &str,
    term_freq_json: &str,
    term_count: i64,
    embedding_model: &str,
    embedding_json: &str,
) -> Result<RagChunk> {
    let now = Utc::now().to_rfc3339();
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO rag_chunks (
            id, chat_id, knowledge_base_id, file_id, file_name, path, chunk_index, content,
            term_freq_json, term_count, embedding_model, embedding_json, created_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            id,
            chat_id,
            knowledge_base_id,
            file_id,
            file_name,
            path,
            chunk_index,
            content,
            term_freq_json,
            term_count,
            embedding_model,
            embedding_json,
            now
        ],
    )?;
    Ok(RagChunk {
        id,
        chat_id: chat_id.map(str::to_string),
        knowledge_base_id: knowledge_base_id.map(str::to_string),
        file_id: file_id.to_string(),
        file_name: file_name.to_string(),
        path: path.to_string(),
        chunk_index,
        content: content.to_string(),
        term_freq_json: term_freq_json.to_string(),
        term_count,
        embedding_model: embedding_model.to_string(),
        embedding_json: embedding_json.to_string(),
        created_at: now,
    })
}

pub fn list_chat_rag_chunks(conn: &Connection, chat_id: &str) -> Result<Vec<RagChunk>> {
    let mut stmt = conn.prepare(
        "SELECT id, chat_id, knowledge_base_id, file_id, file_name, path, chunk_index, content,
                term_freq_json, term_count, embedding_model, embedding_json, created_at
         FROM rag_chunks WHERE chat_id=?1 ORDER BY file_name ASC, chunk_index ASC",
    )?;
    let rows = stmt.query_map(params![chat_id], row_to_rag_chunk)?;
    rows.collect()
}

pub fn list_knowledge_base_rag_chunks(
    conn: &Connection,
    knowledge_base_id: &str,
) -> Result<Vec<RagChunk>> {
    let mut stmt = conn.prepare(
        "SELECT id, chat_id, knowledge_base_id, file_id, file_name, path, chunk_index, content,
                term_freq_json, term_count, embedding_model, embedding_json, created_at
         FROM rag_chunks WHERE knowledge_base_id=?1 ORDER BY file_name ASC, chunk_index ASC",
    )?;
    let rows = stmt.query_map(params![knowledge_base_id], row_to_rag_chunk)?;
    rows.collect()
}

fn row_to_rag_chunk(row: &rusqlite::Row<'_>) -> Result<RagChunk> {
    Ok(RagChunk {
        id: row.get(0)?,
        chat_id: row.get(1)?,
        knowledge_base_id: row.get(2)?,
        file_id: row.get(3)?,
        file_name: row.get(4)?,
        path: row.get(5)?,
        chunk_index: row.get(6)?,
        content: row.get(7)?,
        term_freq_json: row.get(8)?,
        term_count: row.get(9)?,
        embedding_model: row.get(10)?,
        embedding_json: row.get(11)?,
        created_at: row.get(12)?,
    })
}
