use dioxus::document::eval;
use dioxus::prelude::*;
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use crate::db;
use crate::markdown;
use crate::providers::{self, Message};

use super::types::{run_builtin, UiMessage, PROVIDER_OLLAMA};

#[component]
pub fn ChatPane(
    conn: Signal<rusqlite::Connection>,
    chat_id: String,
    mut messages: Signal<Vec<UiMessage>>,
    current_model: Signal<String>,
    current_provider: Signal<String>,
    mut current_system_prompt: Signal<String>,
    ollama_base_url: Signal<String>,
    mut streaming_chats: Signal<HashMap<String, Arc<AtomicBool>>>,
    on_messages_changed: EventHandler<()>,
) -> Element {
    let mut input = use_signal(String::new);

    // Is *this* chat currently streaming?
    let is_streaming = streaming_chats.read().contains_key(&chat_id);

    // System prompt editor state
    let mut sys_prompt_open = use_signal(|| false);
    let mut sys_prompt_draft = use_signal(|| current_system_prompt.read().clone());

    // Keep draft in sync when we switch chats
    use_effect(move || {
        sys_prompt_draft.set(current_system_prompt.read().clone());
    });

    // Scroll to bottom whenever message count changes
    let msg_count = messages.read().len();
    use_effect(move || {
        let _ = msg_count; // read it so effect re-runs on change
        let _ = eval("var a=document.getElementById('scroll-anchor');if(a)a.scrollIntoView({behavior:'smooth'});");
    });

    let commit_sys_prompt = {
        let conn = conn.clone();
        let chat_id = chat_id.clone();
        move || {
            let text = sys_prompt_draft.read().clone();
            current_system_prompt.set(text.clone());
            db::update_chat_system_prompt(&conn.read(), &chat_id, &text).ok();
        }
    };

    let do_stop = {
        let chat_id = chat_id.clone();
        move || {
            if let Some(token) = streaming_chats.read().get(&chat_id) {
                token.store(true, Ordering::Relaxed);
            }
        }
    };

    let do_send = {
        let conn = conn.clone();
        let chat_id = chat_id.clone();
        move || {
            let text = input.read().trim().to_string();
            if text.is_empty() || streaming_chats.read().contains_key(&chat_id) {
                return;
            }

            let model = current_model.read().clone();
            let provider = current_provider.read().clone();
            let system_prompt = current_system_prompt.read().clone();

            if let Ok(user_msg) = db::add_message(&conn.read(), &chat_id, "user", &text) {
                messages.write().push(UiMessage::from_db(&user_msg));
            }

            if messages.read().len() == 1 {
                let title: String = text.chars().take(40).collect();
                db::rename_chat(&conn.read(), &chat_id, &title).ok();
            }

            input.set(String::new());

            if provider == PROVIDER_OLLAMA {
                let mut history: Vec<Message> = Vec::new();
                if !system_prompt.is_empty() {
                    history.push(Message {
                        role: "system".to_string(),
                        content: system_prompt,
                    });
                }
                history.extend(
                    messages
                        .read()
                        .iter()
                        .filter(|m| !m.streaming)
                        .map(|m| Message {
                            role: m.role.clone(),
                            content: m.content.clone(),
                        }),
                );

                let base_url = ollama_base_url.read().clone();
                let stream_id = uuid::Uuid::new_v4().to_string();
                messages.write().push(UiMessage::new_streaming(stream_id.clone()));

                let cancel = Arc::new(AtomicBool::new(false));
                streaming_chats.write().insert(chat_id.clone(), cancel.clone());

                let conn2 = conn.clone();
                let chat_id2 = chat_id.clone();
                spawn(async move {
                    let mut rx = providers::ollama::chat_stream(base_url, model, history);
                    let mut full_content = String::new();

                    loop {
                        if cancel.load(Ordering::Relaxed) {
                            if !full_content.is_empty() {
                                if let Ok(saved) = db::add_message(
                                    &conn2.read(), &chat_id2, "assistant", &full_content,
                                ) {
                                    if let Some(msg) = messages.write().iter_mut().find(|m| m.id == stream_id) {
                                        msg.id = saved.id;
                                        msg.streaming = false;
                                    }
                                }
                            } else {
                                messages.write().retain(|m| m.id != stream_id);
                            }
                            streaming_chats.write().remove(&chat_id2);
                            on_messages_changed.call(());
                            return;
                        }

                        match rx.recv().await {
                            Some(Ok(token)) => {
                                full_content.push_str(&token);
                                let html = markdown::render(&full_content);
                                if let Some(msg) = messages.write().iter_mut().find(|m| m.id == stream_id) {
                                    msg.content = full_content.clone();
                                    msg.html = html;
                                }
                            }
                            Some(Err(e)) => {
                                let err_html = format!("<span style='color:#e03131'>Error: {e}</span>");
                                if let Some(msg) = messages.write().iter_mut().find(|m| m.id == stream_id) {
                                    msg.html = err_html;
                                    msg.streaming = false;
                                }
                                streaming_chats.write().remove(&chat_id2);
                                on_messages_changed.call(());
                                return;
                            }
                            None => break,
                        }
                    }

                    if let Ok(saved) = db::add_message(&conn2.read(), &chat_id2, "assistant", &full_content) {
                        if let Some(msg) = messages.write().iter_mut().find(|m| m.id == stream_id) {
                            msg.id = saved.id;
                            msg.streaming = false;
                        }
                    }
                    streaming_chats.write().remove(&chat_id2);
                    on_messages_changed.call(());
                });
            } else {
                let response_text = run_builtin(&model, &text);
                if let Ok(asst_msg) = db::add_message(&conn.read(), &chat_id, "assistant", &response_text) {
                    messages.write().push(UiMessage::from_db(&asst_msg));
                }
                on_messages_changed.call(());
            }
        }
    };

    rsx! {
        div { id: "chat-pane",

            // ── System prompt bar ──────────────────────────────────────────
            div { id: "system-prompt-bar",
                button {
                    id: "system-prompt-toggle",
                    class: if sys_prompt_open() { "sys-prompt-toggle sys-prompt-toggle--open" } else { "sys-prompt-toggle" },
                    onclick: move |_| *sys_prompt_open.write() ^= true,
                    svg { xmlns: "http://www.w3.org/2000/svg", view_box: "0 0 24 24",
                        fill: "none", stroke: "currentColor", stroke_width: "2",
                        width: "13", height: "13",
                        polyline { points: "6 9 12 15 18 9" }
                    }
                    span {
                        if current_system_prompt.read().is_empty() {
                            "System prompt"
                        } else {
                            "System prompt (set)"
                        }
                    }
                }

                if sys_prompt_open() {
                    div { id: "system-prompt-editor",
                        textarea {
                            id: "system-prompt-input",
                            placeholder: "You are a helpful assistant…",
                            rows: "4",
                            value: "{sys_prompt_draft}",
                            oninput: move |e| sys_prompt_draft.set(e.value()),
                            onblur: {
                                let mut commit = commit_sys_prompt.clone();
                                move |_| commit()
                            },
                        }
                        div { id: "system-prompt-actions",
                            button {
                                class: "accent-btn accent-btn--sm",
                                onclick: {
                                    let mut commit = commit_sys_prompt.clone();
                                    move |_| {
                                        commit();
                                        sys_prompt_open.set(false);
                                    }
                                },
                                "Save"
                            }
                            button {
                                class: "ghost-btn ghost-btn--sm",
                                onclick: move |_| {
                                    sys_prompt_draft.set(current_system_prompt.read().clone());
                                    sys_prompt_open.set(false);
                                },
                                "Cancel"
                            }
                        }
                    }
                }
            }

            // ── Messages ───────────────────────────────────────────────────
            div { id: "chat-messages",
                if messages.read().is_empty() {
                    div { id: "chat-empty",
                        div { id: "chat-empty-icon",
                            svg { xmlns: "http://www.w3.org/2000/svg", view_box: "0 0 24 24",
                                fill: "none", stroke: "currentColor", stroke_width: "1.5",
                                width: "40", height: "40",
                                path { d: "M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" }
                            }
                        }
                        p { "Send a message to start the conversation." }
                        p { class: "chat-empty-sub", "Model: {current_model}" }
                    }
                }

                for msg in messages.read().iter() {
                    {
                        let is_user = msg.role == "user";
                        let html_content = msg.html.clone();
                        let raw_content = msg.content.clone();
                        let msg_id = msg.id.clone();
                        let streaming = msg.streaming;
                        rsx! {
                            div {
                                class: if is_user { "msg-row msg-row--user" } else { "msg-row msg-row--assistant" },
                                key: "{msg_id}",
                                div {
                                    class: if is_user {
                                        "msg-bubble msg-bubble--user"
                                    } else if streaming {
                                        "msg-bubble msg-bubble--assistant msg-bubble--streaming"
                                    } else {
                                        "msg-bubble msg-bubble--assistant"
                                    },
                                    dangerous_inner_html: if html_content.is_empty() && streaming {
                                        "<span class='streaming-cursor'></span>".to_string()
                                    } else {
                                        html_content
                                    },
                                }
                                if !is_user && !streaming {
                                    div { class: "msg-actions",
                                        button {
                                            class: "msg-action-btn",
                                            title: "Copy markdown",
                                            onclick: move |_| {
                                                let _ = eval(&format!(
                                                    "navigator.clipboard.writeText({});",
                                                    serde_json::to_string(&raw_content).unwrap_or_default()
                                                ));
                                            },
                                            svg { xmlns: "http://www.w3.org/2000/svg", view_box: "0 0 24 24",
                                                fill: "none", stroke: "currentColor", stroke_width: "2",
                                                width: "13", height: "13",
                                                rect { x: "9", y: "9", width: "13", height: "13", rx: "2", ry: "2" }
                                                path { d: "M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" }
                                            }
                                            " Copy"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                div { id: "scroll-anchor" }
            }

            // ── Input area ─────────────────────────────────────────────────
            div { id: "chat-input-area",
                div { id: "chat-input-bar",
                    textarea {
                        id: "chat-input",
                        placeholder: "Message {current_model}...",
                        rows: "1",
                        value: "{input}",
                        disabled: is_streaming,
                        oninput: move |e| input.set(e.value()),
                        onkeydown: {
                            let mut do_send = do_send.clone();
                            move |e: KeyboardEvent| {
                                if e.key() == Key::Enter && !e.modifiers().shift() {
                                    e.prevent_default();
                                    do_send();
                                }
                            }
                        },
                    }
                    if is_streaming {
                        button {
                            id: "chat-stop-btn",
                            title: "Stop generation",
                            onclick: move |_| do_stop(),
                            svg { xmlns: "http://www.w3.org/2000/svg", view_box: "0 0 24 24",
                                fill: "currentColor", width: "16", height: "16",
                                rect { x: "4", y: "4", width: "16", height: "16", rx: "2" }
                            }
                        }
                    } else {
                        button {
                            id: "chat-send-btn",
                            disabled: input.read().trim().is_empty(),
                            onclick: {
                                let mut do_send = do_send.clone();
                                move |_| do_send()
                            },
                            svg { xmlns: "http://www.w3.org/2000/svg", view_box: "0 0 24 24",
                                fill: "currentColor", width: "18", height: "18",
                                path { d: "M2.01 21L23 12 2.01 3 2 10l15 2-15 2z" }
                            }
                        }
                    }
                }
                p { id: "input-hint", "Shift+Enter for newline" }
            }
        }
    }
}
