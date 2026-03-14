use dioxus::document::eval;
use dioxus::prelude::*;
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use crate::db::{self, KnowledgeBase, WasmModel};
use crate::markdown;
use crate::providers::{self, ChatAttachment, ChatKnowledgeBaseRef, Message};
use crate::rag;
use crate::tools::{self};

use super::types::{run_builtin, UiMessage, PROVIDER_OLLAMA, PROVIDER_WASI};

fn format_bytes(byte_size: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;

    if byte_size >= MB {
        format!("{:.1} MB", byte_size as f64 / MB as f64)
    } else if byte_size >= KB {
        format!("{:.1} KB", byte_size as f64 / KB as f64)
    } else {
        format!("{byte_size} B")
    }
}

#[component]
pub fn ChatPane(
    conn: Signal<rusqlite::Connection>,
    chat_id: String,
    mut messages: Signal<Vec<UiMessage>>,
    current_model: Signal<String>,
    current_provider: Signal<String>,
    mut current_system_prompt: Signal<String>,
    current_embedding_model: Signal<String>,
    active_tools: Signal<Vec<tools::ChatToolConfig>>,
    ollama_base_url: Signal<String>,
    wasm_models: Signal<Vec<WasmModel>>,
    wasi_apps: Signal<Vec<db::WasiApp>>,
    chat_knowledge_bases: Signal<Vec<KnowledgeBase>>,
    mut streaming_chats: Signal<HashMap<String, Arc<AtomicBool>>>,
    on_open_tool_picker: EventHandler<MouseEvent>,
    on_messages_changed: EventHandler<()>,
) -> Element {
    let mut input = use_signal(String::new);
    let mut uploaded_files: Signal<Vec<db::ChatFile>> =
        use_signal(|| db::list_chat_files(&conn.read(), &chat_id).unwrap_or_default());
    let mut upload_error = use_signal(|| Option::<String>::None);
    let mut uploading_files = use_signal(|| false);
    let mut pending_chat_file_delete: Signal<Option<String>> = use_signal(|| None);
    let mut thinking_expanded = use_signal(|| true);
    let mut citations_expanded = use_signal(|| true);

    let is_streaming = streaming_chats.read().contains_key(&chat_id);

    let mut sys_prompt_open = use_signal(|| false);
    let mut sys_prompt_draft = use_signal(|| current_system_prompt.read().clone());

    use_effect(move || {
        sys_prompt_draft.set(current_system_prompt.read().clone());
    });

    let current_chat_id_for_files = chat_id.clone();
    use_effect(move || {
        uploaded_files
            .set(db::list_chat_files(&conn.read(), &current_chat_id_for_files).unwrap_or_default());
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
            let wasi_apps_list = wasi_apps.read().clone();
            let vfs_json = db::get_chat_vfs(&conn.read(), &chat_id).unwrap_or_default();
            let vfs_handle = tools::vfs_from_json(
                &chat_id,
                &serde_json::to_string(&vfs_json).unwrap_or_default(),
            );

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
                let attachments = db::list_chat_files(&conn.read(), &chat_id)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|file| ChatAttachment {
                        name: file.display_name,
                        path: file.path,
                        inline_context: file.inline_context,
                        is_text: file.is_text,
                    })
                    .collect::<Vec<_>>();
                let knowledge_base_refs = chat_knowledge_bases
                    .read()
                    .iter()
                    .map(|kb| ChatKnowledgeBaseRef {
                        name: kb.name.clone(),
                        description: kb.description.clone(),
                    })
                    .collect::<Vec<_>>();
                let stream_id = uuid::Uuid::new_v4().to_string();
                messages
                    .write()
                    .push(UiMessage::new_streaming(stream_id.clone()));

                let cancel = Arc::new(AtomicBool::new(false));
                streaming_chats
                    .write()
                    .insert(chat_id.clone(), cancel.clone());

                let conn2 = conn.clone();
                let chat_id2 = chat_id.clone();
                let embedding_model = current_embedding_model.read().clone();
                spawn(async move {
                    if cancel.load(Ordering::Relaxed) {
                        messages.write().retain(|m| m.id != stream_id);
                        streaming_chats.write().remove(&chat_id2);
                        on_messages_changed.call(());
                        return;
                    }

                    let retrieved_chunks = rag::retrieve_for_chat(
                        &conn2.read(),
                        &base_url,
                        &chat_id2,
                        &embedding_model,
                        &text,
                        6,
                    )
                    .await
                    .unwrap_or_default();
                    let retrieved_context = rag::format_retrieved_context(&retrieved_chunks);
                    let retrieved_citations = rag::to_message_citations(&retrieved_chunks);

                    let mut rx = providers::ollama::chat_stream(
                        base_url,
                        model,
                        system_prompt,
                        history,
                        text,
                        active_tools_list,
                        wasi_apps_list,
                        vfs_handle,
                        attachments,
                        knowledge_base_refs,
                        retrieved_context,
                    );
                    let mut full_content = String::new();

                    while let Some(chunk) = rx.recv().await {
                        if cancel.load(Ordering::Relaxed) {
                            if !full_content.is_empty() {
                                if let Ok(saved) = db::add_message_with_citations(
                                    &conn2.read(),
                                    &chat_id2,
                                    "assistant",
                                    &full_content,
                                    &retrieved_citations,
                                ) {
                                    if let Some(msg) =
                                        messages.write().iter_mut().find(|m| m.id == stream_id)
                                    {
                                        msg.id = saved.id;
                                        msg.content = full_content.clone();
                                        msg.html = markdown::render(&full_content);
                                        msg.streaming = false;
                                        msg.citations = retrieved_citations.clone();
                                    }
                                }
                            } else {
                                messages.write().retain(|m| m.id != stream_id);
                            }
                            streaming_chats.write().remove(&chat_id2);
                            on_messages_changed.call(());
                            return;
                        }

                        match chunk {
                            Ok(stream_chunk) => {
                                if !stream_chunk.delta.is_empty() {
                                    full_content.push_str(&stream_chunk.delta);
                                    let html = markdown::render(&full_content);
                                    if let Some(msg) =
                                        messages.write().iter_mut().find(|m| m.id == stream_id)
                                    {
                                        msg.content = full_content.clone();
                                        msg.html = html;
                                    }
                                }

                                if let Some(final_content) = stream_chunk.final_content {
                                    if let Ok(saved) = db::add_message_with_citations(
                                        &conn2.read(),
                                        &chat_id2,
                                        "assistant",
                                        &final_content,
                                        &retrieved_citations,
                                    ) {
                                        if let Some(msg) =
                                            messages.write().iter_mut().find(|m| m.id == stream_id)
                                        {
                                            msg.id = saved.id;
                                            msg.content = final_content.clone();
                                            msg.html = markdown::render(&final_content);
                                            msg.streaming = false;
                                            msg.tool_invocations =
                                                stream_chunk.tool_invocations.unwrap_or_default();
                                            msg.citations = stream_chunk
                                                .citations
                                                .unwrap_or_else(|| retrieved_citations.clone());
                                        }
                                    }
                                    streaming_chats.write().remove(&chat_id2);
                                    on_messages_changed.call(());
                                    return;
                                }
                            }
                            Err(e) => {
                                let rendered = format!("Error: {e}");
                                if let Ok(saved) = db::add_message_with_citations(
                                    &conn2.read(),
                                    &chat_id2,
                                    "assistant",
                                    &rendered,
                                    &retrieved_citations,
                                ) {
                                    if let Some(msg) =
                                        messages.write().iter_mut().find(|m| m.id == stream_id)
                                    {
                                        msg.id = saved.id;
                                        msg.content = rendered.clone();
                                        msg.html = format!(
                                            "<span style='color:#e03131'>{rendered}</span>"
                                        );
                                        msg.streaming = false;
                                        msg.citations = retrieved_citations.clone();
                                    }
                                }
                                streaming_chats.write().remove(&chat_id2);
                                on_messages_changed.call(());
                                return;
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
                        let err = format!(
                            "WASI module '{}' not found. Upload it in Provider Settings.",
                            model
                        );
                        if let Ok(asst_msg) =
                            db::add_message(&conn.read(), &chat_id, "assistant", &err)
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
                        history.extend(messages.read().iter().filter(|m| !m.streaming).map(|m| {
                            Message {
                                role: m.role.clone(),
                                content: m.content.clone(),
                            }
                        }));

                        let stream_id = uuid::Uuid::new_v4().to_string();
                        messages
                            .write()
                            .push(UiMessage::new_streaming(stream_id.clone()));

                        let cancel = Arc::new(AtomicBool::new(false));
                        streaming_chats
                            .write()
                            .insert(chat_id.clone(), cancel.clone());

                        let conn2 = conn.clone();
                        let chat_id2 = chat_id.clone();
                        spawn(async move {
                            let mut rx = providers::wasi::chat_stream(
                                wasm_model.file_path,
                                wasm_model.name,
                                history,
                                db::chat_vfs_root(&chat_id2),
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
                                        if let Some(msg) =
                                            messages.write().iter_mut().find(|m| m.id == stream_id)
                                        {
                                            msg.content = full_content.clone();
                                            msg.html = html;
                                        }
                                    }
                                    Some(Err(e)) => {
                                        let err_html = format!(
                                            "<span style='color:#e03131'>WASI Error: {e}</span>"
                                        );
                                        if let Some(msg) =
                                            messages.write().iter_mut().find(|m| m.id == stream_id)
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

                            if let Ok(saved) = db::add_message(
                                &conn2.read(),
                                &chat_id2,
                                "assistant",
                                &full_content,
                            ) {
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
                if let Ok(asst_msg) =
                    db::add_message(&conn.read(), &chat_id, "assistant", &response_text)
                {
                    messages.write().push(UiMessage::from_db(&asst_msg));
                }
                on_messages_changed.call(());
            }
        }
    };

    let on_upload_files = {
        let conn = conn.clone();
        let chat_id = chat_id.clone();
        move |event: FormEvent| {
            let target_chat_id = chat_id.clone();
            let files = event.files();
            if files.is_empty() {
                return;
            }

            upload_error.set(None);
            uploading_files.set(true);
            let base_url = ollama_base_url.read().clone();
            let embedding_model = current_embedding_model.read().clone();
            spawn(async move {
                for file in files {
                    let name = file.name();
                    let mime_type = file.content_type().unwrap_or_default();
                    match file.read_bytes().await {
                        Ok(bytes) => {
                            let extracted_text = rag::extract_text(&bytes, &mime_type);
                            let inline_context = extracted_text
                                .as_deref()
                                .map(rag::inline_context_for_text)
                                .unwrap_or_default();
                            let is_text = extracted_text.is_some();
                            let path = rag::normalize_upload_path(&name);

                            match db::add_chat_file(
                                &conn.read(),
                                &target_chat_id,
                                &path,
                                &name,
                                &mime_type,
                                &bytes,
                                is_text,
                                &inline_context,
                            ) {
                                Ok(saved_file) => {
                                    if is_text && inline_context.is_empty() {
                                        if let Err(err) = rag::index_chat_file(
                                            &conn.read(),
                                            &base_url,
                                            &embedding_model,
                                            &saved_file,
                                        )
                                        .await
                                        {
                                            upload_error.set(Some(format!(
                                                "Failed to index {name}: {err}"
                                            )));
                                        }
                                    }
                                    uploaded_files.write().push(saved_file);
                                }
                                Err(err) => {
                                    upload_error.set(Some(format!("Failed to save {name}: {err}")));
                                }
                            }
                        }
                        Err(err) => {
                            upload_error.set(Some(format!("Failed to read {name}: {err}")));
                        }
                    }
                }
                on_messages_changed.call(());
                uploading_files.set(false);
            });
        }
    };

    let delete_chat_file = {
        let conn = conn.clone();
        move |file_id: String| {
            if db::delete_chat_file(&conn.read(), &file_id).is_ok() {
                uploaded_files.write().retain(|file| file.id != file_id);
                pending_chat_file_delete.set(None);
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
                        let citations = msg.citations.clone();
                        let thinking = msg.thinking.clone();
                        rsx! {
                            div {
                                class: if is_user { "msg-row msg-row--user" } else { "msg-row msg-row--assistant" },
                                key: "{msg_id}",
                                if !is_user && thinking.is_some() {
                                    div { class: "msg-thinking",
                                        div { class: "msg-section-header",
                                            onclick: move |_| *thinking_expanded.write() ^= true,
                                            span { class: if thinking_expanded() { "msg-chevron msg-chevron--expanded" } else { "msg-chevron" },
                                                svg { xmlns: "http://www.w3.org/2000/svg", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", width: "12", height: "12", path { d: "m9 18 6-6-6-6" } }
                                            }
                                            "Thinking"
                                        }
                                        if thinking_expanded() {
                                            div { class: "msg-thinking-content",
                                                "{thinking.as_ref().unwrap()}"
                                            }
                                        }
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
                                if !is_user && !citations.is_empty() {
                                    div { class: "msg-citations",
                                        div { class: "msg-section-header msg-citations-header",
                                            onclick: move |_| *citations_expanded.write() ^= true,
                                            span { class: if citations_expanded() { "msg-chevron msg-chevron--expanded" } else { "msg-chevron" },
                                                svg { xmlns: "http://www.w3.org/2000/svg", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", width: "12", height: "12", path { d: "m9 18 6-6-6-6" } }
                                            }
                                            span { class: "msg-citations-title", "Sources ({citations.len()})" }
                                        }
                                        if citations_expanded() {
                                            div { class: "msg-citations-content",
                                                for (idx, citation) in citations.iter().enumerate() {
                                                    div { class: "msg-citation", key: "citation-{msg_id}-{idx}",
                                                        div { class: "msg-citation-header",
                                                            span { class: "msg-citation-name", "{citation.source_label}" }
                                                            span { class: "msg-citation-path", "{citation.path}" }
                                                        }
                                                        if !citation.excerpt.is_empty() {
                                                            div { class: "msg-citation-excerpt", "{citation.excerpt}" }
                                                        }
                                                    }
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
                            for (idx, tool) in active_tools.read().iter().enumerate() {
                                {
                                    let key = format!("tool-{}-{}", idx, tool.wasi_app_id.as_deref().unwrap_or(tool.kind.id()));
                                    let label = if let Some(ref app_id) = tool.wasi_app_id {
                                        if let Some(app) = wasi_apps.read().iter().find(|a| &a.id == app_id) {
                                            app.name.clone()
                                        } else {
                                            tool.kind.label().to_string()
                                        }
                                    } else {
                                        tool.kind.label().to_string()
                                    };
                                    rsx! {
                                        span { class: "chat-tool-chip", key: "{key}",
                                            "{label}"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                div { id: "chat-meta-row",
                    for kb in chat_knowledge_bases.read().iter() {
                        span { class: "chat-tool-chip chat-kb-chip", key: "kb-{kb.id}",
                            "KB: {kb.name}"
                        }
                    }
                }

                if let Some(error) = upload_error() {
                    p { id: "input-hint", style: "color:#e03131;", "{error}" }
                }

                if !uploaded_files.read().is_empty() {
                    div { id: "chat-file-list",
                        for file in uploaded_files.read().iter() {
                            {
                                let file_id = file.id.clone();
                                let status = if !file.is_text {
                                    "binary"
                                } else if !file.inline_context.is_empty() {
                                    "inline"
                                } else {
                                    "indexed"
                                };
                                let class_name = match status {
                                    "inline" => "chat-file-chip chat-file-chip--inline",
                                    "indexed" => "chat-file-chip chat-file-chip--indexed",
                                    _ => "chat-file-chip",
                                };
                                let is_pending_delete = pending_chat_file_delete.read().as_deref() == Some(&file_id);
                                let size_label = format_bytes(file.byte_size);
                                rsx! {
                                    div { class: class_name, key: "file-{file.id}",
                                        div { class: "chat-file-copy",
                                            div { class: "chat-file-name-row",
                                                span { class: "chat-file-name", "{file.display_name}" }
                                                span { class: "chat-file-meta", "{status}" }
                                            }
                                            div { class: "chat-file-details", "{file.path} - {size_label}" }
                                        }
                                        div { class: "chat-file-actions",
                                            if is_pending_delete {
                                                button {
                                                    class: "ghost-btn ghost-btn--sm chat-file-confirm-btn",
                                                    onclick: {
                                                        let file_id = file_id.clone();
                                                        let mut delete_chat_file = delete_chat_file.clone();
                                                        move |_| delete_chat_file(file_id.clone())
                                                    },
                                                    "Confirm"
                                                }
                                                button {
                                                    class: "ghost-btn ghost-btn--sm",
                                                    onclick: move |_| pending_chat_file_delete.set(None),
                                                    "Cancel"
                                                }
                                            } else {
                                                button {
                                                    class: "chat-file-remove",
                                                    title: "Delete file",
                                                    onclick: {
                                                        let file_id = file_id.clone();
                                                        move |_| pending_chat_file_delete.set(Some(file_id.clone()))
                                                    },
                                                    svg { xmlns: "http://www.w3.org/2000/svg", view_box: "0 0 24 24",
                                                        fill: "none", stroke: "currentColor", stroke_width: "2",
                                                        width: "12", height: "12",
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
                }

                div { id: "chat-input-bar",
                    label {
                        id: "chat-file-attach-btn",
                        title: "Attach files",
                        r#for: "chat-file-input",
                        svg { xmlns: "http://www.w3.org/2000/svg", view_box: "0 0 24 24",
                            fill: "none", stroke: "currentColor", stroke_width: "2",
                            width: "18", height: "18",
                            path { d: "M21.44 11.05l-9.19 9.19a6 6 0 0 1-8.49-8.49l9.19-9.19a4 4 0 0 1 5.66 5.66l-9.2 9.19a2 2 0 0 1-2.83-2.83l8.49-8.48" }
                        }
                    }
                    input {
                        id: "chat-file-input",
                        r#type: "file",
                        multiple: true,
                        style: "display:none",
                        onchange: on_upload_files,
                    }
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
                p { id: "input-hint", "Shift+Enter for newline. Short text files are added directly to context; larger text files are indexed for retrieval." }
            }
        }
    }
}
