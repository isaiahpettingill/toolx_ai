use crate::db::DbMessage;
use crate::markdown;

// ── Provider names ────────────────────────────────────────────────────────────

pub const PROVIDER_BUILTIN: &str = "builtin";
pub const PROVIDER_OLLAMA: &str = "ollama";
pub const PROVIDER_WASI: &str = "wasi";

// ── Built-in (local) models ───────────────────────────────────────────────────

#[derive(Clone, PartialEq, Debug)]
pub struct BuiltinModel {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
}

pub const BUILTIN_MODELS: &[BuiltinModel] = &[
    BuiltinModel {
        id: "echo:0b",
        label: "echo:0b",
        description: "Echoes your input back",
    },
    BuiltinModel {
        id: "reverse:0b",
        label: "reverse:0b",
        description: "Reverses your input",
    },
];

pub fn run_builtin(model_id: &str, input: &str) -> String {
    match model_id {
        "reverse:0b" => input.chars().rev().collect(),
        _ => input.to_string(),
    }
}

// ── In-memory message state ───────────────────────────────────────────────────

#[derive(Clone, PartialEq, Debug)]
pub struct UiMessage {
    pub id: String,
    pub role: String,
    pub content: String,
    pub html: String,
    pub streaming: bool,
}

impl UiMessage {
    pub fn from_db(msg: &DbMessage) -> Self {
        let html = if msg.role == "assistant" {
            markdown::render(&msg.content)
        } else {
            escape_user_text(&msg.content)
        };
        UiMessage {
            id: msg.id.clone(),
            role: msg.role.clone(),
            content: msg.content.clone(),
            html,
            streaming: false,
        }
    }

    pub fn new_streaming(id: String) -> Self {
        UiMessage {
            id,
            role: "assistant".to_string(),
            content: String::new(),
            html: String::new(),
            streaming: true,
        }
    }
}

pub fn escape_user_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
