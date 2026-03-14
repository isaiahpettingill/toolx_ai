use dioxus::prelude::*;
use std::collections::HashMap;

use crate::db::{self, KnowledgeBase, KnowledgeBaseFile, WasiApp, WasmModel};
use crate::providers;
use crate::rag;
use crate::tools;

const SETTING_OLLAMA_URL: &str = "ollama_base_url";

// ── Provider config panel ─────────────────────────────────────────────────────

#[component]
pub fn ProviderConfigPanel(
    conn: Signal<rusqlite::Connection>,
    mut ollama_base_url: Signal<String>,
    mut wasm_models: Signal<Vec<WasmModel>>,
    mut wasi_apps: Signal<Vec<WasiApp>>,
    mut knowledge_bases: Signal<Vec<KnowledgeBase>>,
    on_close: EventHandler<()>,
) -> Element {
    let mut active_tab: Signal<&'static str> = use_signal(|| "ollama");

    rsx! {
        div {
            id: "config-backdrop",
            onclick: move |_| on_close.call(()),
        }

        div { id: "config-panel",
            div { id: "config-panel-header",
                span { id: "config-panel-title", "Provider Settings" }
                button {
                    id: "config-panel-close",
                    title: "Close",
                    onclick: move |_| on_close.call(()),
                    svg { xmlns: "http://www.w3.org/2000/svg", view_box: "0 0 24 24",
                        fill: "none", stroke: "currentColor", stroke_width: "2",
                        width: "16", height: "16",
                        line { x1: "18", y1: "6", x2: "6", y2: "18" }
                        line { x1: "6", y1: "6", x2: "18", y2: "18" }
                    }
                }
            }

            // ── Provider tabs ──────────────────────────────────────────────
            div { id: "config-provider-tabs",
                button {
                    class: if active_tab() == "ollama" { "config-tab config-tab--active" } else { "config-tab" },
                    onclick: move |_| active_tab.set("ollama"),
                    // monitor icon
                    svg { xmlns: "http://www.w3.org/2000/svg", view_box: "0 0 24 24",
                        fill: "none", stroke: "currentColor", stroke_width: "2",
                        width: "14", height: "14",
                        rect { x: "2", y: "3", width: "20", height: "14", rx: "2" }
                        line { x1: "8", y1: "21", x2: "16", y2: "21" }
                        line { x1: "12", y1: "17", x2: "12", y2: "21" }
                    }
                    " Ollama"
                }
                button {
                    class: if active_tab() == "wasi" { "config-tab config-tab--active" } else { "config-tab" },
                    onclick: move |_| active_tab.set("wasi"),
                    // cpu / chip icon
                    svg { xmlns: "http://www.w3.org/2000/svg", view_box: "0 0 24 24",
                        fill: "none", stroke: "currentColor", stroke_width: "2",
                        width: "14", height: "14",
                        rect { x: "9", y: "9", width: "6", height: "6" }
                        path { d: "M9 3H7a2 2 0 0 0-2 2v2M15 3h2a2 2 0 0 1 2 2v2M9 21H7a2 2 0 0 1-2-2v-2M15 21h2a2 2 0 0 0 2-2v-2M3 9v2M3 13v2M21 9v2M21 13v2" }
                    }
                    " WASI Modules"
                }
                button {
                    class: if active_tab() == "knowledge" { "config-tab config-tab--active" } else { "config-tab" },
                    onclick: move |_| active_tab.set("knowledge"),
                    svg { xmlns: "http://www.w3.org/2000/svg", view_box: "0 0 24 24",
                        fill: "none", stroke: "currentColor", stroke_width: "2",
                        width: "14", height: "14",
                        path { d: "M4 19.5A2.5 2.5 0 0 1 6.5 17H20" }
                        path { d: "M6.5 2H20v20H6.5A2.5 2.5 0 0 1 4 19.5v-15A2.5 2.5 0 0 1 6.5 2z" }
                    }
                    " Knowledge"
                }
                button {
                    class: if active_tab() == "wasi_apps" { "config-tab config-tab--active" } else { "config-tab" },
                    onclick: move |_| active_tab.set("wasi_apps"),
                    // terminal icon
                    svg { xmlns: "http://www.w3.org/2000/svg", view_box: "0 0 24 24",
                        fill: "none", stroke: "currentColor", stroke_width: "2",
                        width: "14", height: "14",
                        polyline { points: "4 17 10 11 4 5" }
                        line { x1: "12", y1: "19", x2: "20", y2: "19" }
                    }
                    " WASI Apps"
                }
            }

            div { id: "config-panel-body",
                if active_tab() == "ollama" {
                    OllamaSection { conn, ollama_base_url, on_close }
                } else if active_tab() == "knowledge" {
                    KnowledgeBasesSection { conn, ollama_base_url, knowledge_bases }
                } else if active_tab() == "wasi_apps" {
                    WasiAppsSection { conn, wasi_apps }
                } else {
                    WasiSection { conn, wasm_models }
                }
            }
        }
    }
}

#[component]
fn KnowledgeBasesSection(
    conn: Signal<rusqlite::Connection>,
    ollama_base_url: Signal<String>,
    mut knowledge_bases: Signal<Vec<KnowledgeBase>>,
) -> Element {
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

    let mut name = use_signal(String::new);
    let mut description = use_signal(String::new);
    let available_models = rag::default_embedding_models();
    let mut embedding_model = use_signal(|| available_models.first().cloned().unwrap_or_default());
    let mut upload_error = use_signal(|| Option::<String>::None);
    let mut uploading_kb: Signal<Option<String>> = use_signal(|| None);
    let mut pending_kb_file_delete: Signal<Option<String>> = use_signal(|| None);
    let mut kb_files: Signal<HashMap<String, Vec<KnowledgeBaseFile>>> = use_signal(HashMap::new);

    use_effect(move || {
        let mut next = HashMap::new();
        for kb in knowledge_bases.read().iter() {
            next.insert(
                kb.id.clone(),
                db::list_knowledge_base_files(&conn.read(), &kb.id).unwrap_or_default(),
            );
        }
        kb_files.set(next);
    });

    let mut refresh_kb_files = {
        let conn = conn.clone();
        move |knowledge_base_id: &str| {
            kb_files.write().insert(
                knowledge_base_id.to_string(),
                db::list_knowledge_base_files(&conn.read(), knowledge_base_id).unwrap_or_default(),
            );
        }
    };

    rsx! {
        div { class: "config-section",
            div { class: "config-section-header",
                div { class: "config-section-icon",
                    svg { xmlns: "http://www.w3.org/2000/svg", view_box: "0 0 24 24",
                        fill: "none", stroke: "currentColor", stroke_width: "2",
                        width: "16", height: "16",
                        path { d: "M4 19.5A2.5 2.5 0 0 1 6.5 17H20" }
                        path { d: "M6.5 2H20v20H6.5A2.5 2.5 0 0 1 4 19.5v-15A2.5 2.5 0 0 1 6.5 2z" }
                    }
                }
                div {
                    div { class: "config-section-name", "Knowledge Bases" }
                    div { class: "config-section-desc", "Create reusable document collections and attach them to chats." }
                }
            }

            div { class: "config-field",
                label { class: "config-label", "Name" }
                input { class: "config-input", value: "{name}", oninput: move |e| name.set(e.value()) }
            }
            div { class: "config-field",
                label { class: "config-label", "Description" }
                textarea {
                    class: "config-input",
                    rows: "3",
                    value: "{description}",
                    oninput: move |e| description.set(e.value()),
                }
            }
            div { class: "config-field",
                label { class: "config-label", "Embeddings Model" }
                select {
                    class: "config-input",
                    value: "{embedding_model}",
                    onchange: move |e| embedding_model.set(e.value()),
                    for model in available_models.iter() {
                        option { value: "{model}", "{model}" }
                    }
                }
            }
            div { class: "config-actions",
                button {
                    class: "accent-btn",
                    disabled: name.read().trim().is_empty(),
                    onclick: move |_| {
                        let name_value = name.read().trim().to_string();
                        let description_value = description.read().clone();
                        let embedding_model_value = embedding_model.read().clone();
                        if name_value.is_empty() {
                            return;
                        }
                        match db::create_knowledge_base(
                            &conn.read(),
                            &name_value,
                            &description_value,
                            &embedding_model_value,
                        ) {
                            Ok(kb) => {
                                knowledge_bases.write().insert(0, kb);
                                name.set(String::new());
                                description.set(String::new());
                            }
                            Err(err) => upload_error.set(Some(format!("Failed to create knowledge base: {err}"))),
                        }
                    },
                    "Create knowledge base"
                }
            }

            if let Some(err) = upload_error() {
                div { class: "config-test-err", "{err}" }
            }

            if knowledge_bases.read().is_empty() {
                div { class: "wasi-empty", "No knowledge bases yet." }
            } else {
                div { class: "wasi-module-list",
                    for kb in knowledge_bases.read().clone().into_iter() {
                        {
                            let kb_id = kb.id.clone();
                            let kb_id_delete = kb.id.clone();
                            let kb_name = kb.name.clone();
                            let file_input_id = format!("kb-upload-{}", kb.id);
                            rsx! {
                                div { class: "wasi-module-row", key: "{kb_id}",
                                    div { class: "wasi-module-icon",
                                        svg { xmlns: "http://www.w3.org/2000/svg", view_box: "0 0 24 24",
                                            fill: "none", stroke: "currentColor", stroke_width: "2",
                                            width: "16", height: "16",
                                            path { d: "M4 19.5A2.5 2.5 0 0 1 6.5 17H20" }
                                            path { d: "M6.5 2H20v20H6.5A2.5 2.5 0 0 1 4 19.5v-15A2.5 2.5 0 0 1 6.5 2z" }
                                        }
                                    }
                                    div { class: "wasi-module-info",
                                        div { class: "wasi-module-name", "{kb.name}" }
                                        div { class: "wasi-module-meta",
                                            span { class: "wasi-badge", "KB" }
                                            span { class: "wasi-module-size", "{kb.embedding_model}" }
                                        }
                                        if !kb.description.is_empty() {
                                            div { class: "wasi-module-desc", "{kb.description}" }
                                        }
                                        div { class: "tool-card-hint", "Upload text files to build the RAG index." }
                                    }
                                    label {
                                        class: "ghost-btn ghost-btn--sm",
                                        r#for: "{file_input_id}",
                                        if uploading_kb.read().as_deref() == Some(&kb_id) { "Uploading…" } else { "Add file" }
                                    }
                                    input {
                                        id: "{file_input_id}",
                                        r#type: "file",
                                        style: "display:none",
                                        onchange: move |e| {
                                            let files = e.files();
                                            let conn = conn.clone();
                                            let kb_id_inner = kb_id.clone();
                                            let kb_name_inner = kb_name.clone();
                                            let kb_embedding_model = kb.embedding_model.clone();
                                            let base_url = ollama_base_url.read().clone();
                                            uploading_kb.set(Some(kb_id_inner.clone()));
                                            upload_error.set(None);
                                             spawn(async move {
                                                 for file in files {
                                                    let file_name = file.name();
                                                    match file.read_bytes().await {
                                                        Ok(bytes) => {
                                                            let mime = file.content_type().unwrap_or_default();
                                                            let is_text = rag::extract_text(&bytes, &mime).is_some();
                                                            match db::add_knowledge_base_file(
                                                                &conn.read(),
                                                                &kb_id_inner,
                                                                &rag::normalize_upload_path(&file_name),
                                                                &file_name,
                                                                &mime,
                                                                &bytes,
                                                                is_text,
                                                            ) {
                                                                Ok(file_record) => {
                                                                    if is_text {
                                                                        if let Err(err) = rag::index_knowledge_base_file(
                                                                            &conn.read(),
                                                                            &base_url,
                                                                            &kb_embedding_model,
                                                                            &file_record,
                                                                        ).await {
                                                                            upload_error.set(Some(format!("Failed to index {}: {}", kb_name_inner, err)));
                                                                        }
                                                                    }
                                                                }
                                                                Err(err) => upload_error.set(Some(format!("Upload failed: {err}"))),
                                                            }
                                                        }
                                                        Err(err) => upload_error.set(Some(format!("Failed to read file: {err}"))),
                                                    }
                                                 }
                                                 knowledge_bases.set(db::list_knowledge_bases(&conn.read()).unwrap_or_default());
                                                 kb_files.write().insert(
                                                     kb_id_inner.clone(),
                                                     db::list_knowledge_base_files(&conn.read(), &kb_id_inner).unwrap_or_default(),
                                                 );
                                                 uploading_kb.set(None);
                                             });
                                         },
                                    }
                                    if let Some(files) = kb_files.read().get(&kb.id).cloned() {
                                        if !files.is_empty() {
                                            div { class: "kb-file-list",
                                                for file in files {
                                                    {
                                                        let file_id = file.id.clone();
                                                        let kb_id_for_file = kb.id.clone();
                                                        let status = if file.is_text { "indexed" } else { "binary" };
                                                        let size_label = format_bytes(file.byte_size);
                                                        let is_pending_delete = pending_kb_file_delete.read().as_deref() == Some(&file_id);
                                                        rsx! {
                                                            div { class: "kb-file-row", key: "kb-file-{file.id}",
                                                                div { class: "kb-file-copy",
                                                                    div { class: "kb-file-name", "{file.display_name}" }
                                                                    div { class: "kb-file-meta", "{status} - {size_label} - {file.path}" }
                                                                }
                                                                div { class: "chat-file-actions",
                                                                    if is_pending_delete {
                                                                        button {
                                                                            class: "ghost-btn ghost-btn--sm chat-file-confirm-btn",
                                                                            onclick: {
                                                                                let file_id = file_id.clone();
                                                                                let kb_id_for_file = kb_id_for_file.clone();
                                                                                move |_| {
                                                                                    if db::delete_knowledge_base_file(&conn.read(), &file_id).is_ok() {
                                                                                        refresh_kb_files(&kb_id_for_file);
                                                                                        pending_kb_file_delete.set(None);
                                                                                    }
                                                                                }
                                                                            },
                                                                            "Confirm"
                                                                        }
                                                                        button {
                                                                            class: "ghost-btn ghost-btn--sm",
                                                                            onclick: move |_| pending_kb_file_delete.set(None),
                                                                            "Cancel"
                                                                        }
                                                                    } else {
                                                                        button {
                                                                            class: "icon-btn icon-btn--danger",
                                                                            title: "Delete file",
                                                                            onclick: {
                                                                                let file_id = file_id.clone();
                                                                                move |_| pending_kb_file_delete.set(Some(file_id.clone()))
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
                                        }
                                    }
                                    button {
                                        class: "icon-btn icon-btn--danger",
                                        title: "Delete knowledge base",
                                        onclick: move |_| {
                                            db::delete_knowledge_base(&conn.read(), &kb_id_delete).ok();
                                            knowledge_bases.write().retain(|item| item.id != kb_id_delete);
                                            kb_files.write().remove(&kb_id_delete);
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
    }
}

// ── Ollama section ────────────────────────────────────────────────────────────

#[component]
fn OllamaSection(
    conn: Signal<rusqlite::Connection>,
    mut ollama_base_url: Signal<String>,
    on_close: EventHandler<()>,
) -> Element {
    let mut url_draft = use_signal(|| ollama_base_url.read().clone());
    let mut test_result: Signal<Option<Result<(), String>>> = use_signal(|| None);
    let mut testing = use_signal(|| false);

    rsx! {
        div { class: "config-section",
            div { class: "config-section-header",
                div { class: "config-section-icon",
                    svg { xmlns: "http://www.w3.org/2000/svg", view_box: "0 0 24 24",
                        fill: "none", stroke: "currentColor", stroke_width: "2",
                        width: "16", height: "16",
                        rect { x: "2", y: "3", width: "20", height: "14", rx: "2" }
                        line { x1: "8", y1: "21", x2: "16", y2: "21" }
                        line { x1: "12", y1: "17", x2: "12", y2: "21" }
                    }
                }
                div {
                    div { class: "config-section-name", "Ollama" }
                    div { class: "config-section-desc", "Local models via Ollama" }
                }
            }

            div { class: "config-field",
                label { class: "config-label", "Base URL" }
                div { class: "config-input-row",
                    input {
                        class: "config-input",
                        r#type: "url",
                        value: "{url_draft}",
                        placeholder: "http://localhost:11434",
                        oninput: move |e| {
                            url_draft.set(e.value());
                            test_result.set(None);
                        },
                    }
                    button {
                        class: if testing() { "config-test-btn config-test-btn--loading" } else { "config-test-btn" },
                        disabled: testing(),
                        onclick: move |_| {
                            let url = url_draft.read().clone();
                            testing.set(true);
                            test_result.set(None);
                            spawn(async move {
                                match providers::ollama::list_models(&url).await {
                                    Ok(_) => test_result.set(Some(Ok(()))),
                                    Err(e) => test_result.set(Some(Err(e.to_string()))),
                                }
                                testing.set(false);
                            });
                        },
                        if testing() { "Testing…" } else { "Test" }
                    }
                }

                if let Some(ref result) = *test_result.read() {
                    match result {
                        Ok(()) => rsx! { div { class: "config-test-ok", "Connected successfully" } },
                        Err(msg) => {
                            let msg = msg.clone();
                            rsx! { div { class: "config-test-err", "{msg}" } }
                        }
                    }
                }
            }

            div { class: "config-actions",
                button {
                    class: "accent-btn",
                    onclick: move |_| {
                        let new_url = url_draft.read().trim().to_string();
                        if !new_url.is_empty() {
                            ollama_base_url.set(new_url.clone());
                            db::set_setting(&conn.read(), SETTING_OLLAMA_URL, &new_url).ok();
                        }
                        on_close.call(());
                    },
                    "Save"
                }
                button {
                    class: "ghost-btn",
                    onclick: move |_| on_close.call(()),
                    "Cancel"
                }
            }
        }
    }
}

// ── WASI module manager section ───────────────────────────────────────────────

#[component]
fn WasiSection(
    conn: Signal<rusqlite::Connection>,
    mut wasm_models: Signal<Vec<WasmModel>>,
) -> Element {
    let mut upload_error: Signal<Option<String>> = use_signal(|| None);
    let mut uploading = use_signal(|| false);

    rsx! {
        div { class: "config-section",
            div { class: "config-section-header",
                div { class: "config-section-icon",
                    svg { xmlns: "http://www.w3.org/2000/svg", view_box: "0 0 24 24",
                        fill: "none", stroke: "currentColor", stroke_width: "2",
                        width: "16", height: "16",
                        rect { x: "9", y: "9", width: "6", height: "6" }
                        path { d: "M9 3H7a2 2 0 0 0-2 2v2M15 3h2a2 2 0 0 1 2 2v2M9 21H7a2 2 0 0 1-2-2v-2M15 21h2a2 2 0 0 0 2-2v-2M3 9v2M3 13v2M21 9v2M21 13v2" }
                    }
                }
                div {
                    div { class: "config-section-name", "WASI Modules" }
                    div { class: "config-section-desc",
                        "Upload "
                        code { "wasm32-wasip1" }
                        " binaries. Chat with them via stdin/stdout."
                    }
                }
            }

            // Upload area
            div { class: "wasi-upload-area",
                label { class: "wasi-upload-label", r#for: "wasm-file-input",
                    svg { xmlns: "http://www.w3.org/2000/svg", view_box: "0 0 24 24",
                        fill: "none", stroke: "currentColor", stroke_width: "2",
                        width: "20", height: "20",
                        path { d: "M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" }
                        polyline { points: "17 8 12 3 7 8" }
                        line { x1: "12", y1: "3", x2: "12", y2: "15" }
                    }
                    if uploading() {
                        span { "Uploading…" }
                    } else {
                        span { "Click to upload a " }
                        code { ".wasm" }
                        span { " module" }
                    }
                }
                input {
                    id: "wasm-file-input",
                    r#type: "file",
                    accept: ".wasm",
                    style: "display:none",
                    onchange: move |e| {
                        upload_error.set(None);
                        uploading.set(true);
                        let file_list = e.files();
                        spawn(async move {
                            if let Some(file) = file_list.first() {
                                let name = file.name();
                                match file.read_bytes().await {
                                    Ok(bytes) => {
                                        match db::add_wasm_model(&conn.read(), &name, &bytes) {
                                            Ok(model) => {
                                                wasm_models.write().push(model);
                                            }
                                            Err(e) => {
                                                upload_error.set(Some(format!("DB error: {e}")));
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        upload_error.set(Some(format!("Failed to read file: {e}")));
                                    }
                                }
                            }
                            uploading.set(false);
                        });
                    },
                }
            }

            if let Some(ref err) = *upload_error.read() {
                div { class: "config-test-err", "{err}" }
            }

            // Module list
            if wasm_models.read().is_empty() {
                div { class: "wasi-empty",
                    "No WASI modules uploaded yet."
                }
            } else {
                div { class: "wasi-module-list",
                    for model in wasm_models.read().clone().into_iter() {
                        {
                            let model_id = model.id.clone();
                            let model_name = model.name.clone();
                            let size_kb = model.file_size / 1024;
                            rsx! {
                                div { class: "wasi-module-row", key: "{model_id}",
                                    div { class: "wasi-module-icon",
                                        svg { xmlns: "http://www.w3.org/2000/svg", view_box: "0 0 24 24",
                                            fill: "none", stroke: "currentColor", stroke_width: "2",
                                            width: "16", height: "16",
                                            path { d: "M13 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9z" }
                                            polyline { points: "13 2 13 9 20 9" }
                                        }
                                    }
                                    div { class: "wasi-module-info",
                                        div { class: "wasi-module-name", "{model_name}" }
                                        div { class: "wasi-module-meta",
                                            span { class: "wasi-badge", "WASI" }
                                            span { class: "wasi-module-size", "{size_kb} KB" }
                                        }
                                    }
                                    button {
                                        class: "icon-btn icon-btn--danger",
                                        title: "Remove module",
                                        onclick: move |_| {
                                            db::delete_wasm_model(&conn.read(), &model_id).ok();
                                            wasm_models.write().retain(|m| m.id != model_id);
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
    }
}

// ── WASI Apps section ─────────────────────────────────────────────────────────

#[component]
fn WasiAppsSection(
    conn: Signal<rusqlite::Connection>,
    mut wasi_apps: Signal<Vec<WasiApp>>,
) -> Element {
    let mut uploading = use_signal(|| false);
    let mut upload_error = use_signal(|| Option::<String>::None);
    let mut editing_id: Signal<Option<String>> = use_signal(|| None);
    let mut edit_buf = use_signal(String::new);

    rsx! {
        div { class: "config-section",
            div { class: "config-section-header",
                div { class: "config-section-icon",
                    svg { xmlns: "http://www.w3.org/2000/svg", view_box: "0 0 24 24",
                        fill: "none", stroke: "currentColor", stroke_width: "2",
                        width: "16", height: "16",
                        polyline { points: "4 17 10 11 4 5" }
                        line { x1: "12", y1: "19", x2: "20", y2: "19" }
                    }
                }
                div {
                    div { class: "config-section-name", "WASI CLI Apps" }
                    div { class: "config-section-desc",
                        "Upload "
                        code { "wasm32-wasip1" }
                        " CLI binaries. Run with arguments, auto-generate help."
                    }
                }
            }

            // Upload area
            div { class: "wasi-upload-area",
                label { class: "wasi-upload-label", r#for: "wasi-app-file-input",
                    svg { xmlns: "http://www.w3.org/2000/svg", view_box: "0 0 24 24",
                        fill: "none", stroke: "currentColor", stroke_width: "2",
                        width: "20", height: "20",
                        path { d: "M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" }
                        polyline { points: "17 8 12 3 7 8" }
                        line { x1: "12", y1: "3", x2: "12", y2: "15" }
                    }
                    if uploading() {
                        span { "Uploading…" }
                    } else {
                        span { "Click to upload a " }
                        code { ".wasm" }
                        span { " CLI app" }
                    }
                }
                input {
                    id: "wasi-app-file-input",
                    r#type: "file",
                    accept: ".wasm",
                    style: "display:none",
                    onchange: move |e| {
                        upload_error.set(None);
                        uploading.set(true);
                        let file_list = e.files();
                        let conn = conn.clone();
                        spawn(async move {
                            if let Some(file) = file_list.first() {
                                let name = file.name();
                                match file.read_bytes().await {
                                    Ok(bytes) => {
                                        let help_text = tools::generate_help_text(&bytes, &name).await;
                                        let description = if help_text.len() > 100 {
                                            format!("{}...", &help_text[..100])
                                        } else {
                                            help_text.clone()
                                        };

                                        match db::add_wasi_app(&conn.read(), &name, &description, &help_text, &bytes) {
                                            Ok(app) => {
                                                wasi_apps.write().push(app);
                                            }
                                            Err(e) => {
                                                upload_error.set(Some(format!("DB error: {e}")));
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        upload_error.set(Some(format!("Failed to read file: {e}")));
                                    }
                                }
                            }
                            uploading.set(false);
                        });
                    },
                }
            }

            if let Some(ref err) = *upload_error.read() {
                div { class: "config-test-err", "{err}" }
            }

            // Apps list
            if wasi_apps.read().is_empty() {
                div { class: "wasi-empty",
                    "No WASI apps uploaded yet."
                }
            } else {
                div { class: "wasi-module-list",
                    for (idx, app) in wasi_apps.read().clone().into_iter().enumerate() {
                        {
                            let app_id = app.id.clone();
                            let app_id_for_edit = app.id.clone();
                            let app_id_for_delete = app.id.clone();
                            let app_name = app.name.clone();
                            let app_desc = app.description.clone();
                            let size_kb = app.file_size / 1024;
                            let is_editing = editing_id.read().as_deref() == Some(&app_id);
                            rsx! {
                                div { class: "wasi-module-row", key: "wasi-app-{idx}-{app_id}",
                                    if is_editing {
                                        input {
                                            class: "config-input",
                                            value: "{edit_buf}",
                                            autofocus: true,
                                            oninput: move |e| edit_buf.set(e.value()),
                                            onkeydown: {
                                                let conn = conn.clone();
                                                let app_id_inner = app_id.clone();
                                                move |e: KeyboardEvent| {
                                                    if e.key() == Key::Enter {
                                                        let new_desc = edit_buf.read().clone();
                                                        db::update_wasi_app(&conn.read(), &app_id_inner, &new_desc).ok();
                                                        if let Some(app) = wasi_apps.write().iter_mut().find(|a| a.id == app_id_inner) {
                                                            app.description = new_desc;
                                                        }
                                                        editing_id.set(None);
                                                    } else if e.key() == Key::Escape {
                                                        editing_id.set(None);
                                                    }
                                                }
                                            },
                                            onblur: {
                                                let conn = conn.clone();
                                                let app_id_inner = app_id.clone();
                                                move |_| {
                                                    let new_desc = edit_buf.read().clone();
                                                    db::update_wasi_app(&conn.read(), &app_id_inner, &new_desc).ok();
                                                    if let Some(app) = wasi_apps.write().iter_mut().find(|a| a.id == app_id_inner) {
                                                        app.description = new_desc;
                                                    }
                                                    editing_id.set(None);
                                                }
                                            },
                                        }
                                    } else {
                                        div { class: "wasi-module-icon",
                                            svg { xmlns: "http://www.w3.org/2000/svg", view_box: "0 0 24 24",
                                                fill: "none", stroke: "currentColor", stroke_width: "2",
                                                width: "16", height: "16",
                                                polyline { points: "4 17 10 11 4 5" }
                                                line { x1: "12", y1: "19", x2: "20", y2: "19" }
                                            }
                                        }
                                        div { class: "wasi-module-info",
                                            div { class: "wasi-module-name", "{app_name}" }
                                            div { class: "wasi-module-meta",
                                                span { class: "wasi-badge", "CLI" }
                                                span { class: "wasi-module-size", "{size_kb} KB" }
                                            }
                                            if !app_desc.is_empty() {
                                                div { class: "wasi-module-desc", "{app_desc}" }
                                            }
                                        }
                                        button {
                                            class: "icon-btn",
                                            title: "Edit description",
                                            onclick: move |_| {
                                                edit_buf.set(app_desc.clone());
                                                editing_id.set(Some(app_id_for_edit.clone()));
                                            },
                                            svg { xmlns: "http://www.w3.org/2000/svg", view_box: "0 0 24 24",
                                                fill: "none", stroke: "currentColor", stroke_width: "2",
                                                width: "13", height: "13",
                                                path { d: "M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7" }
                                                path { d: "M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z" }
                                            }
                                        }
                                    }
                                    button {
                                        class: "icon-btn icon-btn--danger",
                                        title: "Remove app",
                                        onclick: move |_| {
                                            db::delete_wasi_app(&conn.read(), &app_id_for_delete).ok();
                                            wasi_apps.write().retain(|a| a.id != app_id_for_delete);
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
    }
}

// ── Color picker ──────────────────────────────────────────────────────────────

const PRESET_COLORS: &[(&str, &str)] = &[
    ("Indigo", "#3b5bdb"),
    ("Violet", "#7048e8"),
    ("Teal", "#0ca678"),
    ("Rose", "#e03131"),
    ("Amber", "#f08c00"),
    ("Sky", "#1098ad"),
];

#[component]
pub fn ColorPicker(mut accent: Signal<String>) -> Element {
    rsx! {
        div { id: "color-picker",
            span { id: "color-picker-label", "Accent color" }
            div { id: "color-swatches",
                for (name, hex) in PRESET_COLORS.iter() {
                    {
                        let hex_str = hex.to_string();
                        let hex_str2 = hex.to_string();
                        let is_active = accent.read().as_str() == *hex;
                        rsx! {
                            button {
                                class: if is_active { "color-swatch color-swatch--active" } else { "color-swatch" },
                                title: "{name}",
                                style: "background:{hex_str};",
                                onclick: move |_| accent.set(hex_str2.clone()),
                            }
                        }
                    }
                }
                label { class: "color-custom-wrap", title: "Custom color",
                    input {
                        r#type: "color",
                        class: "color-custom-input",
                        value: "{accent}",
                        oninput: move |e| accent.set(e.value()),
                    }
                    svg { xmlns: "http://www.w3.org/2000/svg", view_box: "0 0 24 24",
                        fill: "none", stroke: "currentColor", stroke_width: "2",
                        width: "14", height: "14",
                        circle { cx: "12", cy: "12", r: "3" }
                        path { d: "M19.07 4.93a10 10 0 0 1 0 14.14" }
                        path { d: "M4.93 4.93a10 10 0 0 0 0 14.14" }
                    }
                }
            }
        }
    }
}
