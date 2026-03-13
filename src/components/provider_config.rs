use dioxus::prelude::*;

use crate::db;
use crate::providers;

const SETTING_OLLAMA_URL: &str = "ollama_base_url";

// ── Provider config panel ─────────────────────────────────────────────────────

#[component]
pub fn ProviderConfigPanel(
    conn: Signal<rusqlite::Connection>,
    mut ollama_base_url: Signal<String>,
    on_close: EventHandler<()>,
) -> Element {
    let mut url_draft = use_signal(|| ollama_base_url.read().clone());
    let mut test_result: Signal<Option<Result<(), String>>> = use_signal(|| None);
    let mut testing = use_signal(|| false);

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

            div { id: "config-panel-body",
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
                                Err(msg) => { let msg = msg.clone(); rsx! { div { class: "config-test-err", "{msg}" } } }
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
