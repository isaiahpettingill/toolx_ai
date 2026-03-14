use dioxus::prelude::*;

use crate::db::{self, WasmModel};
use crate::providers;

use super::types::{BUILTIN_MODELS, PROVIDER_BUILTIN, PROVIDER_OLLAMA, PROVIDER_WASI};

#[component]
pub fn ModelSelector(
    conn: Signal<rusqlite::Connection>,
    mut current_model: Signal<String>,
    mut current_provider: Signal<String>,
    mut ollama_base_url: Signal<String>,
    wasm_models: Signal<Vec<WasmModel>>,
    chat_id: Option<String>,
    on_open_provider_config: EventHandler<()>,
) -> Element {
    let mut model_open = use_signal(|| false);

    let ollama_models: Resource<Vec<providers::RemoteModel>> = use_resource(move || {
        let url = ollama_base_url();
        async move {
            providers::ollama::list_models(&url)
                .await
                .unwrap_or_default()
        }
    });

    let is_ollama = current_provider() == PROVIDER_OLLAMA;
    let is_wasi = current_provider() == PROVIDER_WASI;

    let chat_id_builtin = chat_id.clone();
    let chat_id_ollama = chat_id.clone();
    let chat_id_wasi = chat_id.clone();

    rsx! {
        div { id: "chat-topbar",

            div { id: "provider-tabs",
                button {
                    class: if !is_ollama && !is_wasi { "provider-tab provider-tab--active" } else { "provider-tab" },
                    onclick: move |_| {
                        current_provider.set(PROVIDER_BUILTIN.to_string());
                        current_model.set(BUILTIN_MODELS[0].id.to_string());
                        if let Some(ref id) = chat_id_builtin {
                            db::update_chat_provider(&conn.read(), id, PROVIDER_BUILTIN).ok();
                            db::update_chat_model(&conn.read(), id, BUILTIN_MODELS[0].id).ok();
                        }
                        model_open.set(false);
                    },
                    "Built-in"
                }
                button {
                    class: if is_ollama { "provider-tab provider-tab--active" } else { "provider-tab" },
                    onclick: move |_| {
                        current_provider.set(PROVIDER_OLLAMA.to_string());
                        if let Some(ref id) = chat_id_ollama {
                            db::update_chat_provider(&conn.read(), id, PROVIDER_OLLAMA).ok();
                        }
                        model_open.set(false);
                    },
                    "Ollama"
                }
                button {
                    class: if is_wasi { "provider-tab provider-tab--active" } else { "provider-tab" },
                    onclick: move |_| {
                        current_provider.set(PROVIDER_WASI.to_string());
                        // Auto-select the first WASI module if available
                        if let Some(first) = wasm_models.read().first() {
                            current_model.set(first.id.clone());
                            if let Some(ref id) = chat_id_wasi {
                                db::update_chat_provider(&conn.read(), id, PROVIDER_WASI).ok();
                                db::update_chat_model(&conn.read(), id, &first.id).ok();
                            }
                        } else if let Some(ref id) = chat_id_wasi {
                            db::update_chat_provider(&conn.read(), id, PROVIDER_WASI).ok();
                        }
                        model_open.set(false);
                    },
                    "WASI"
                }
            }

            div { id: "model-selector",
                button {
                    id: "model-selector-btn",
                    onclick: move |_| *model_open.write() ^= true,
                    span {
                        // Show a friendly name: for WASI show the module filename, else show raw id
                        if is_wasi {
                            if let Some(m) = wasm_models.read().iter().find(|m| m.id == current_model()) {
                                if m.name.is_empty() {
                                    "{current_model()}"
                                } else {
                                    "{m.name}"
                                }
                            } else {
                                "{current_model()}"
                            }
                        } else {
                            "{current_model()}"
                        }
                    }
                    svg { xmlns: "http://www.w3.org/2000/svg", view_box: "0 0 24 24",
                        fill: "none", stroke: "currentColor", stroke_width: "2",
                        width: "12", height: "12",
                        polyline { points: "6 9 12 15 18 9" }
                    }
                }

                if model_open() {
                    div { id: "model-dropdown",
                        if is_wasi {
                            if wasm_models.read().is_empty() {
                                div { class: "model-option model-option--empty",
                                    div { "No WASI modules uploaded" }
                                    div { class: "model-option-hint", "Upload a .wasm file in Provider Settings" }
                                    button {
                                        class: "model-option-config-link",
                                        onclick: move |_| {
                                            model_open.set(false);
                                            on_open_provider_config.call(());
                                        },
                                        "Open Provider Settings →"
                                    }
                                }
                            } else {
                                for m in wasm_models.read().clone().into_iter() {
                                    {
                                        let mid = m.id.clone();
                                        let mlabel = if m.name.is_empty() { m.id.clone() } else { m.name.clone() };
                                        let size_kb = m.bytes.len() / 1024;
                                        let conn2 = conn.clone();
                                        let cid = chat_id.clone();
                                        let is_active = current_model() == mid;
                                        rsx! {
                                            button {
                                                class: if is_active { "model-option model-option--active" } else { "model-option" },
                                                key: "{mid}",
                                                onclick: move |_| {
                                                    current_model.set(mid.clone());
                                                    if let Some(ref id) = cid {
                                                        db::update_chat_model(&conn2.read(), id, &mid).ok();
                                                    }
                                                    model_open.set(false);
                                                },
                                                div { class: "model-option-name", "{mlabel}" }
                                                div { class: "model-option-desc",
                                                    span { class: "wasi-badge", "WASI" }
                                                    " {size_kb} KB"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        } else if is_ollama {
                            match ollama_models() {
                                None => rsx! {
                                    div { class: "model-option model-option--loading", "Fetching models…" }
                                },
                                Some(models) if models.is_empty() => rsx! {
                                    div { class: "model-option model-option--empty",
                                        div { "No models found" }
                                        div { class: "model-option-hint", "Is Ollama running?" }
                                        button {
                                            class: "model-option-config-link",
                                            onclick: move |_| {
                                                model_open.set(false);
                                                on_open_provider_config.call(());
                                            },
                                            "Configure Ollama →"
                                        }
                                    }
                                },
                                Some(models) => rsx! {
                                    for m in models.iter() {
                                        {
                                            let mid = m.id.clone();
                                            let mlabel = m.label.clone();
                                            let conn2 = conn.clone();
                                            let cid = chat_id.clone();
                                            let is_active = current_model() == mid;
                                            rsx! {
                                                button {
                                                    class: if is_active { "model-option model-option--active" } else { "model-option" },
                                                    key: "{mid}",
                                                    onclick: move |_| {
                                                        current_model.set(mid.clone());
                                                        if let Some(ref id) = cid {
                                                            db::update_chat_model(&conn2.read(), id, &mid).ok();
                                                        }
                                                        model_open.set(false);
                                                    },
                                                    div { class: "model-option-name", "{mlabel}" }
                                                }
                                            }
                                        }
                                    }
                                },
                            }
                        } else {
                            for m in BUILTIN_MODELS.iter() {
                                {
                                    let mid = m.id;
                                    let mlabel = m.label;
                                    let mdesc = m.description;
                                    let conn2 = conn.clone();
                                    let cid = chat_id.clone();
                                    let is_active = current_model() == mid;
                                    rsx! {
                                        button {
                                            class: if is_active { "model-option model-option--active" } else { "model-option" },
                                            key: "{mid}",
                                            onclick: move |_| {
                                                current_model.set(mid.to_string());
                                                if let Some(ref id) = cid {
                                                    db::update_chat_model(&conn2.read(), id, mid).ok();
                                                }
                                                model_open.set(false);
                                            },
                                            div { class: "model-option-name", "{mlabel}" }
                                            div { class: "model-option-desc", "{mdesc}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            div { style: "flex:1" }

            button {
                id: "topbar-config-btn",
                title: "Provider settings",
                onclick: move |_| on_open_provider_config.call(()),
                svg { xmlns: "http://www.w3.org/2000/svg", view_box: "0 0 24 24",
                    fill: "none", stroke: "currentColor", stroke_width: "2",
                    width: "15", height: "15",
                    circle { cx: "12", cy: "12", r: "3" }
                    path { d: "M19.07 4.93a10 10 0 0 1 0 14.14M4.93 4.93a10 10 0 0 0 0 14.14" }
                    path { d: "M12 2v2M12 20v2M2 12h2M20 12h2" }
                }
            }
        }
    }
}
