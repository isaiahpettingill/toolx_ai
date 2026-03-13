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

fn db_path() -> PathBuf {
    let mut p = dirs_next();
    p.push("toolx_ai");
    std::fs::create_dir_all(&p).ok();
    p.push("chats.db");
    p
}

fn dirs_next() -> PathBuf {
    // Use the data dir or fall back to current dir
    if let Some(d) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        let mut p = PathBuf::from(d);
        p.push(".local/share");
        p
    } else {
        PathBuf::from(".")
    }
}

pub fn open() -> Result<Connection> {
    let conn = Connection::open(db_path())?;
    conn.execute_batch("PRAGMA journal_mode=WAL;")?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS chats (
            id            TEXT PRIMARY KEY,
            title         TEXT NOT NULL,
            model         TEXT NOT NULL DEFAULT 'echo:0b',
            provider      TEXT NOT NULL DEFAULT 'builtin',
            system_prompt TEXT NOT NULL DEFAULT '',
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
        );",
    )?;
    // Migrations for existing DBs
    conn.execute_batch("ALTER TABLE chats ADD COLUMN provider TEXT NOT NULL DEFAULT 'builtin';")
        .ok();
    conn.execute_batch("ALTER TABLE chats ADD COLUMN system_prompt TEXT NOT NULL DEFAULT '';")
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
        "SELECT id, title, model, provider, system_prompt, created_at, updated_at FROM chats ORDER BY updated_at DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(ChatSummary {
            id: row.get(0)?,
            title: row.get(1)?,
            model: row.get(2)?,
            provider: row.get(3)?,
            system_prompt: row.get(4)?,
            created_at: row.get(5)?,
            updated_at: row.get(6)?,
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
        "INSERT INTO chats (id, title, model, provider, system_prompt, created_at, updated_at) VALUES (?1,?2,?3,?4,'',?5,?6)",
        params![id, title, model, provider, now, now],
    )?;
    Ok(ChatSummary {
        id,
        title: title.to_string(),
        model: model.to_string(),
        provider: provider.to_string(),
        system_prompt: String::new(),
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
