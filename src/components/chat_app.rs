use dioxus::prelude::*;
use std::collections::HashMap;
use std::sync::{atomic::AtomicBool, Arc};

use crate::db::{self, ChatSummary};
use crate::providers;

use super::chat_pane::ChatPane;
use super::model_selector::ModelSelector;
use super::provider_config::{ColorPicker, ProviderConfigPanel};
use super::types::{UiMessage, PROVIDER_BUILTIN};

const CHAT_CSS: Asset = asset!("/assets/styling/chat.css");
const SETTING_OLLAMA_URL: &str = "ollama_base_url";

// ── Root App component ────────────────────────────────────────────────────────

#[component]
pub fn ChatApp() -> Element {
    let accent = use_signal(|| "#3b5bdb".to_string());
    use_context_provider(|| accent);

    let conn = use_signal(|| db::open().expect("Failed to open SQLite database"));

    let ollama_base_url = use_signal(|| {
        db::get_setting(&conn.read(), SETTING_OLLAMA_URL)
            .ok()
            .flatten()
            .unwrap_or_else(|| providers::ollama::DEFAULT_BASE_URL.to_string())
    });

    let mut chats: Signal<Vec<ChatSummary>> =
        use_signal(|| db::list_chats(&conn.read()).unwrap_or_default());

    let mut active_chat_id: Signal<Option<String>> = use_signal(|| {
        db::list_chats(&conn.read())
            .unwrap_or_default()
            .into_iter()
            .next()
            .map(|c| c.id)
    });

    let mut messages: Signal<Vec<UiMessage>> = use_signal(Vec::new);
    let mut current_model = use_signal(|| "echo:0b".to_string());
    let mut current_provider = use_signal(|| PROVIDER_BUILTIN.to_string());
    let mut current_system_prompt = use_signal(String::new);

    // Per-chat streaming cancel tokens. Presence = that chat is actively streaming.
    let streaming_chats: Signal<HashMap<String, Arc<AtomicBool>>> = use_signal(HashMap::new);
    use_context_provider(|| streaming_chats);

    let mut drawer_open = use_signal(|| false);
    let mut provider_config_open = use_signal(|| false);

    let mut renaming_id: Signal<Option<String>> = use_signal(|| None);
    let mut rename_buf = use_signal(|| String::new());

    // Load messages + chat meta whenever active chat changes
    use_effect(move || {
        if let Some(id) = active_chat_id.read().clone() {
            let conn_r = conn.read();
            let msgs = db::get_messages(&conn_r, &id).unwrap_or_default();
            messages.set(msgs.iter().map(UiMessage::from_db).collect());
            if let Some(chat) = db::list_chats(&conn_r)
                .unwrap_or_default()
                .into_iter()
                .find(|c| c.id == id)
            {
                current_model.set(chat.model.clone());
                current_provider.set(chat.provider.clone());
                current_system_prompt.set(chat.system_prompt.clone());
            }
        }
    });

    let new_chat = {
        let conn = conn.clone();
        move |_: MouseEvent| {
            let model = current_model.read().clone();
            let provider = current_provider.read().clone();
            match db::create_chat(&conn.read(), "New chat", &model, &provider) {
                Ok(chat) => {
                    let id = chat.id.clone();
                    chats.write().insert(0, chat);
                    active_chat_id.set(Some(id));
                    messages.set(Vec::new());
                    current_system_prompt.set(String::new());
                    drawer_open.set(false);
                }
                Err(e) => eprintln!("Failed to create chat: {e}"),
            }
        }
    };

    rsx! {
        document::Link { rel: "stylesheet", href: CHAT_CSS }

        div { id: "app-shell",
            style: "--accent: {accent}",

            AppHeader {
                drawer_open: drawer_open(),
                on_hamburger: move |_| *drawer_open.write() ^= true,
                on_new_chat: new_chat,
            }

            div { id: "app-body",

                if drawer_open() {
                    div {
                        id: "drawer-backdrop",
                        onclick: move |_| drawer_open.set(false),
                    }
                }

                div {
                    id: "sidebar",
                    class: if drawer_open() { "drawer-open" } else { "" },

                    div { id: "sidebar-header",
                        span { id: "sidebar-brand", "toolx ai" }
                        button {
                            id: "new-chat-btn",
                            title: "New chat",
                            onclick: new_chat,
                            svg { xmlns: "http://www.w3.org/2000/svg", view_box: "0 0 24 24",
                                fill: "none", stroke: "currentColor", stroke_width: "2",
                                width: "15", height: "15",
                                path { d: "M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7" }
                                path { d: "M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z" }
                            }
                        }
                    }

                    div { id: "sidebar-chats",
                        for chat in chats.read().iter() {
                            {
                                let chat_id = chat.id.clone();
                                let chat_id2 = chat.id.clone();
                                let chat_id3 = chat.id.clone();
                                let is_active = active_chat_id.read().as_deref() == Some(&chat.id);
                                let is_streaming = streaming_chats.read().contains_key(&chat.id);
                                let is_renaming = renaming_id.read().as_deref() == Some(&chat.id);
                                let title = chat.title.clone();
                                let mut row_class = if is_active {
                                    "chat-row chat-row--active".to_string()
                                } else {
                                    "chat-row".to_string()
                                };
                                if is_streaming { row_class.push_str(" chat-row--streaming"); }
                                rsx! {
                                    div {
                                        class: "{row_class}",
                                        key: "{chat_id}",
                                        onclick: move |_| {
                                            let conn_r = conn.read();
                                            let msgs = db::get_messages(&conn_r, &chat_id).unwrap_or_default();
                                            messages.set(msgs.iter().map(UiMessage::from_db).collect());
                                            if let Some(c) = db::list_chats(&conn_r)
                                                .unwrap_or_default()
                                                .into_iter()
                                                .find(|c| c.id == chat_id)
                                            {
                                                current_model.set(c.model.clone());
                                                current_provider.set(c.provider.clone());
                                                current_system_prompt.set(c.system_prompt.clone());
                                            }
                                            active_chat_id.set(Some(chat_id.clone()));
                                            drawer_open.set(false);
                                        },
                                        if is_renaming {
                                            input {
                                                class: "rename-input",
                                                value: "{rename_buf}",
                                                autofocus: true,
                                                oninput: move |e| rename_buf.set(e.value()),
                                                onkeydown: {
                                                    let conn = conn.clone();
                                                    move |e: KeyboardEvent| {
                                                        if e.key() == Key::Enter || e.key() == Key::Escape {
                                                            commit_rename(&conn, &mut renaming_id, &mut rename_buf, &mut chats);
                                                        }
                                                    }
                                                },
                                                onblur: {
                                                    let conn = conn.clone();
                                                    move |_| {
                                                        commit_rename(&conn, &mut renaming_id, &mut rename_buf, &mut chats);
                                                    }
                                                },
                                            }
                                        } else {
                                            span { class: "chat-row-title", "{title}" }
                                            if is_streaming {
                                                div { class: "chat-row-spinner" }
                                            }
                                            div { class: "chat-row-actions",
                                                button {
                                                    class: "icon-btn",
                                                    title: "Rename",
                                                    onclick: move |e| {
                                                        e.stop_propagation();
                                                        rename_buf.set(
                                                            chats.read().iter().find(|c| c.id == chat_id2)
                                                                .map(|c| c.title.clone())
                                                                .unwrap_or_default()
                                                        );
                                                        renaming_id.set(Some(chat_id2.clone()));
                                                    },
                                                    svg { xmlns: "http://www.w3.org/2000/svg", view_box: "0 0 24 24",
                                                        fill: "none", stroke: "currentColor", stroke_width: "2",
                                                        width: "13", height: "13",
                                                        path { d: "M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7" }
                                                        path { d: "M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z" }
                                                    }
                                                }
                                                button {
                                                    class: "icon-btn icon-btn--danger",
                                                    title: "Delete",
                                                    onclick: move |e| {
                                                        e.stop_propagation();
                                                        db::delete_chat(&conn.read(), &chat_id3).ok();
                                                        chats.write().retain(|c| c.id != chat_id3);
                                                        if active_chat_id.read().as_deref() == Some(&chat_id3) {
                                                            let first = chats.read().first().map(|c| c.id.clone());
                                                            active_chat_id.set(first.clone());
                                                            if let Some(new_id) = &first {
                                                                let msgs = db::get_messages(&conn.read(), new_id).unwrap_or_default();
                                                                messages.set(msgs.iter().map(UiMessage::from_db).collect());
                                                            } else {
                                                                messages.set(Vec::new());
                                                            }
                                                        }
                                                    },
                                                    svg { xmlns: "http://www.w3.org/2000/svg", view_box: "0 0 24 24",
                                                        fill: "none", stroke: "currentColor", stroke_width: "2",
                                                        width: "13", height: "13",
                                                        polyline { points: "3 6 5 6 21 6" }
                                                        path { d: "M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6" }
                                                        path { d: "M10 11v6" }
                                                        path { d: "M14 11v6" }
                                                        path { d: "M9 6V4h6v2" }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    div { id: "sidebar-footer",
                        button {
                            id: "provider-settings-btn",
                            class: if provider_config_open() {
                                "sidebar-footer-btn sidebar-footer-btn--active"
                            } else {
                                "sidebar-footer-btn"
                            },
                            title: "Provider settings",
                            onclick: move |_| *provider_config_open.write() ^= true,
                            svg { xmlns: "http://www.w3.org/2000/svg", view_box: "0 0 24 24",
                                fill: "none", stroke: "currentColor", stroke_width: "2",
                                width: "15", height: "15",
                                circle { cx: "12", cy: "12", r: "3" }
                                path { d: "M19.07 4.93a10 10 0 0 1 0 14.14M4.93 4.93a10 10 0 0 0 0 14.14" }
                                path { d: "M12 2v2M12 20v2M2 12h2M20 12h2" }
                            }
                            span { "Providers" }
                        }
                        ColorPicker { accent }
                    }
                }

                div { id: "main-area",

                    ModelSelector {
                        conn,
                        current_model,
                        current_provider,
                        ollama_base_url,
                        chat_id: active_chat_id().clone(),
                        on_open_provider_config: move |_| provider_config_open.set(true),
                    }

                    if let Some(cid) = active_chat_id() {
                        ChatPane {
                            conn,
                            chat_id: cid,
                            messages,
                            current_model,
                            current_provider,
                            current_system_prompt,
                            ollama_base_url,
                            streaming_chats,
                            on_messages_changed: move |_| {
                                chats.set(db::list_chats(&conn.read()).unwrap_or_default());
                            },
                        }
                    } else {
                        div { id: "no-chat",
                            svg { xmlns: "http://www.w3.org/2000/svg", view_box: "0 0 24 24",
                                fill: "none", stroke: "currentColor", stroke_width: "1.5",
                                width: "40", height: "40", opacity: "0.3",
                                path { d: "M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" }
                            }
                            p { "No chats yet." }
                            button {
                                class: "accent-btn",
                                onclick: new_chat,
                                "New chat"
                            }
                        }
                    }
                }

                if provider_config_open() {
                    ProviderConfigPanel {
                        conn,
                        ollama_base_url,
                        on_close: move |_| provider_config_open.set(false),
                    }
                }
            }
        }
    }
}

// ── Rename helper ─────────────────────────────────────────────────────────────

fn commit_rename(
    conn: &Signal<rusqlite::Connection>,
    renaming_id: &mut Signal<Option<String>>,
    rename_buf: &mut Signal<String>,
    chats: &mut Signal<Vec<ChatSummary>>,
) {
    if let Some(id) = renaming_id.read().clone() {
        let title = rename_buf.read().trim().to_string();
        if !title.is_empty() {
            db::rename_chat(&conn.read(), &id, &title).ok();
            if let Some(c) = chats.write().iter_mut().find(|c| c.id == id) {
                c.title = title;
            }
        }
    }
    renaming_id.set(None);
}

// ── Mobile header bar ─────────────────────────────────────────────────────────

#[component]
fn AppHeader(
    drawer_open: bool,
    on_hamburger: EventHandler<MouseEvent>,
    on_new_chat: EventHandler<MouseEvent>,
) -> Element {
    rsx! {
        header { id: "app-header",
            button {
                id: "hamburger-btn",
                class: if drawer_open { "hamburger-btn hamburger-btn--open" } else { "hamburger-btn" },
                title: "Toggle chats",
                onclick: on_hamburger,
                div { class: "hamburger-line" }
                div { class: "hamburger-line" }
                div { class: "hamburger-line" }
            }
            span { id: "header-brand", "toolx ai" }
            div { id: "header-right",
                button {
                    id: "header-new-chat-btn",
                    title: "New chat",
                    onclick: on_new_chat,
                    svg { xmlns: "http://www.w3.org/2000/svg", view_box: "0 0 24 24",
                        fill: "none", stroke: "currentColor", stroke_width: "2",
                        width: "16", height: "16",
                        line { x1: "12", y1: "5", x2: "12", y2: "19" }
                        line { x1: "5", y1: "12", x2: "19", y2: "12" }
                    }
                }
            }
        }
    }
}
