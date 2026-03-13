use dioxus::document::eval;
use dioxus::prelude::*;
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use crate::db::{self, WasmModel};
use crate::markdown;
use crate::providers::{self, Message};
use crate::tools;

use super::types::{run_builtin, UiMessage, PROVIDER_OLLAMA, PROVIDER_WASI};

#[component]
pub fn ChatPane(
    conn: Signal<rusqlite::Connection>,
    chat_id: String,
    mut messages: Signal<Vec<UiMessage>>,
    current_model: Signal<String>,
    current_provider: Signal<String>,
    mut current_system_prompt: Signal<String>,
    active_tools: Signal<Vec<tools::ChatToolConfig>>,
    ollama_base_url: Signal<String>,
    wasm_models: Signal<Vec<WasmModel>>,
    mut streaming_chats: Signal<HashMap<String, Arc<AtomicBool>>>,
    on_open_tool_picker: EventHandler<MouseEvent>,
    on_messages_changed: EventHandler<()>,
) -> Element {
    let mut input = use_signal(String::new);

    let is_streaming = streaming_chats.read().contains_key(&chat_id);

    let mut sys_prompt_open = use_signal(|| false);
    let mut sys_prompt_draft = use_signal(|| current_system_prompt.read().clone());

    use_effect(move || {
        sys_prompt_draft.set(current_system_prompt.read().clone());
    });

    let msg_count = messages.read().len();
    let current_chat_id = chat_id.clone();
    use_effect(move || {
        let _ = current_chat_id;
        let _ = msg_count;
        let _ = eval(
            "var a=document.getElementById('scroll-anchor');if(a)a.scrollIntoView({behavior:'smooth'});",
        );
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
            let active_tools_list = active_tools.read().clone();

            if let Ok(user_msg) = db::add_message(&conn.read(), &chat_id, "user", &text) {
                messages.write().push(UiMessage::from_db(&user_msg));
            }

            if messages.read().len() == 1 {
                let title: String = text.chars().take(40).collect();
                db::rename_chat(&conn.read(), &chat_id, &title).ok();
            }

            input.set(String::new());

            if provider == PROVIDER_OLLAMA {
                let history: Vec<Message> = messages
                    .read()
                    .iter()
                    .filter(|m| !m.streaming)
                    .map(|m| Message {
                        role: m.role.clone(),
                        content: m.content.clone(),
                    })
                    .collect();

                let base_url = ollama_base_url.read().clone();
                let stream_id = uuid::Uuid::new_v4().to_string();
                messages.write().push(UiMessage::new_streaming(stream_id.clone()));

                let cancel = Arc::new(AtomicBool::new(false));
                streaming_chats.write().insert(chat_id.clone(), cancel.clone());

                let conn2 = conn.clone();
                let chat_id2 = chat_id.clone();
                spawn(async move {
                    if cancel.load(Ordering::Relaxed) {
                        messages.write().retain(|m| m.id != stream_id);
                        streaming_chats.write().remove(&chat_id2);
                        on_messages_changed.call(());
                        return;
                    }

                    match providers::ollama::chat(
                        base_url,
                        model,
                        system_prompt,
                        history,
                        text,
                        active_tools_list,
                    )
                    .await
                    {
                        Ok(result) => {
                            eprintln!("[DEBUG] Raw response: {}", result.content);
                            if let Ok(saved) =
                                db::add_message(&conn2.read(), &chat_id2, "assistant", &result.content)
                            {
                                if let Some(msg) =
                                    messages.write().iter_mut().find(|m| m.id == stream_id)
                                {
                                    msg.id = saved.id;
                                    msg.content = result.content.clone();
                                    msg.html = markdown::render(&result.content);
                                    msg.streaming = false;
                                    msg.tool_invocations = result.tool_invocations;
                                }
                            }
                        }
                        Err(e) => {
                            let rendered = format!("Error: {e}");
                            if let Ok(saved) =
                                db::add_message(&conn2.read(), &chat_id2, "assistant", &rendered)
                            {
                                if let Some(msg) =
                                    messages.write().iter_mut().find(|m| m.id == stream_id)
                                {
                                    msg.id = saved.id;
                                    msg.content = rendered.clone();
                                    msg.html =
                                        format!("<span style='color:#e03131'>{rendered}</span>");
                                    msg.streaming = false;
                                }
                            }
                        }
                    }

                    streaming_chats.write().remove(&chat_id2);
                    on_messages_changed.call(());
                });
            } else if provider == PROVIDER_WASI {
                let wasm_entry = wasm_models.read().iter().find(|m| m.id == model).cloned();
                match wasm_entry {
                    None => {
                        let err =
                            format!("WASI module '{}' not found. Upload it in Provider Settings.", model);
                        if let Ok(asst_msg) = db::add_message(&conn.read(), &chat_id, "assistant", &err)
                        {
                            messages.write().push(UiMessage::from_db(&asst_msg));
                        }
                        on_messages_changed.call(());
                    }
                    Some(wasm_model) => {
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

                        let stream_id = uuid::Uuid::new_v4().to_string();
                        messages.write().push(UiMessage::new_streaming(stream_id.clone()));

                        let cancel = Arc::new(AtomicBool::new(false));
                        streaming_chats.write().insert(chat_id.clone(), cancel.clone());

                        let conn2 = conn.clone();
                        let chat_id2 = chat_id.clone();
                        spawn(async move {
                            let mut rx = providers::wasi::chat_stream(
                                wasm_model.bytes,
                                wasm_model.name,
                                history,
                            );
                            let mut full_content = String::new();

                            loop {
                                if cancel.load(Ordering::Relaxed) {
                                    if !full_content.is_empty() {
                                        if let Ok(saved) = db::add_message(
                                            &conn2.read(),
                                            &chat_id2,
                                            "assistant",
                                            &full_content,
                                        ) {
                                            if let Some(msg) = messages
                                                .write()
                                                .iter_mut()
                                                .find(|m| m.id == stream_id)
                                            {
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
                                        if let Some(msg) = messages
                                            .write()
                                            .iter_mut()
                                            .find(|m| m.id == stream_id)
                                        {
                                            msg.content = full_content.clone();
                                            msg.html = html;
                                        }
                                    }
                                    Some(Err(e)) => {
                                        let err_html =
                                            format!("<span style='color:#e03131'>WASI Error: {e}</span>");
                                        if let Some(msg) = messages
                                            .write()
                                            .iter_mut()
                                            .find(|m| m.id == stream_id)
                                        {
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

                            if let Ok(saved) =
                                db::add_message(&conn2.read(), &chat_id2, "assistant", &full_content)
                            {
                                if let Some(msg) =
                                    messages.write().iter_mut().find(|m| m.id == stream_id)
                                {
                                    msg.id = saved.id;
                                    msg.streaming = false;
                                }
                            }
                            streaming_chats.write().remove(&chat_id2);
                            on_messages_changed.call(());
                        });
                    }
                }
            } else {
                let response_text = run_builtin(&model, &text);
                if let Ok(asst_msg) = db::add_message(&conn.read(), &chat_id, "assistant", &response_text)
                {
                    messages.write().push(UiMessage::from_db(&asst_msg));
                }
                on_messages_changed.call(());
            }
        }
    };

    rsx! {
        div { id: "chat-pane",

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
                        let tool_invocations = msg.tool_invocations.clone();
                        let thinking = msg.thinking.clone();
                        rsx! {
                            div {
                                class: if is_user { "msg-row msg-row--user" } else { "msg-row msg-row--assistant" },
                                key: "{msg_id}",
                                if !is_user && thinking.is_some() {
                                    div { class: "msg-thinking",
                                        "{thinking.as_ref().unwrap()}"
                                    }
                                }
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
                                if !is_user && !tool_invocations.is_empty() {
                                    div { class: "msg-tool-invocations",
                                        for (idx, invocation) in tool_invocations.iter().enumerate() {
                                            div { class: "tool-invocation", key: "tool-{idx}",
                                                div { class: "tool-invocation-name",
                                                    svg { xmlns: "http://www.w3.org/2000/svg", view_box: "0 0 24 24",
                                                        fill: "none", stroke: "currentColor", stroke_width: "2",
                                                        width: "12", height: "12",
                                                        circle { cx: "11", cy: "11", r: "8" }
                                                        path { d: "m21 21-4.35-4.35" }
                                                    }
                                                    "{invocation.tool_name}"
                                                }
                                                div { class: "tool-invocation-query",
                                                    "{invocation.query}"
                                                }
                                            }
                                        }
                                    }
                                }
                                if !is_user && !streaming {
                                    div { class: "msg-actions",
                                        button {
                                            class: "msg-action-btn",
                                            title: "Copy markdown",
                                            onclick: move |_| {
                                                let js_text = serde_json::to_string(&raw_content).unwrap_or_default();
                                                let _ = eval(&format!(
                                                    r#"(function(t){{
                                                        if(navigator.clipboard&&navigator.clipboard.writeText){{
                                                            navigator.clipboard.writeText(t);
                                                        }}else{{
                                                            var el=document.createElement('textarea');
                                                            el.value=t;
                                                            el.style.position='fixed';
                                                            el.style.opacity='0';
                                                            document.body.appendChild(el);
                                                            el.focus();el.select();
                                                            try{{document.execCommand('copy');}}catch(e){{}}
                                                            document.body.removeChild(el);
                                                        }}
                                                    }})({})"#,
                                                    js_text
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

            div { id: "chat-input-area",
                div { id: "chat-tools-row",
                    button {
                        id: "chat-tools-trigger",
                        title: "Add tools to this chat",
                        onclick: move |event| on_open_tool_picker.call(event),
                        svg { xmlns: "http://www.w3.org/2000/svg", view_box: "0 0 24 24",
                            fill: "none", stroke: "currentColor", stroke_width: "2",
                            width: "15", height: "15",
                            path { d: "M12 5v14" }
                            path { d: "M5 12h14" }
                        }
                        span { "Tools" }
                    }

                    div { id: "chat-tools-chips",
                        if active_tools.read().is_empty() {
                            span { class: "chat-tools-empty", "No tools enabled" }
                        } else {
                            for tool in active_tools.read().iter() {
                                span { class: "chat-tool-chip", key: "{tool.kind.id()}",
                                    "{tool.kind.label()}"
                                }
                            }
                        }
                    }
                }

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
                p { id: "input-hint", "Shift+Enter for newline. Enabled tools are available to Ollama automatically." }
            }
        }
    }
}
