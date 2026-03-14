use dioxus::prelude::*;

use crate::db::WasiApp;
use crate::tools::{ChatToolConfig, ChatToolKind, AVAILABLE_TOOLS};

#[component]
pub fn ToolPickerModal(
    active_tools: Signal<Vec<ChatToolConfig>>,
    wasi_apps: Signal<Vec<WasiApp>>,
    on_toggle_tool: EventHandler<ChatToolKind>,
    on_toggle_wasi: EventHandler<String>,
    on_close: EventHandler<()>,
) -> Element {
    let has_wasi = !wasi_apps.read().is_empty();

    rsx! {
        div {
            id: "tool-picker-backdrop",
            onclick: move |_| on_close.call(()),
        }

        div {
            id: "tool-picker-modal",

            div { id: "tool-picker-header",
                div {
                    p { class: "tool-picker-eyebrow", "Chat tools" }
                    h2 { id: "tool-picker-title", "Add tools to this chat" }
                    p { id: "tool-picker-subtitle", "Enable lightweight tools that can be invoked by the AI." }
                }
                button {
                    id: "tool-picker-close",
                    title: "Close tool picker",
                    onclick: move |_| on_close.call(()),
                    svg { xmlns: "http://www.w3.org/2000/svg", view_box: "0 0 24 24",
                        fill: "none", stroke: "currentColor", stroke_width: "2",
                        width: "16", height: "16",
                        line { x1: "18", y1: "6", x2: "6", y2: "18" }
                        line { x1: "6", y1: "6", x2: "18", y2: "18" }
                    }
                }
            }

            div { id: "tool-picker-body",
                // Built-in tools
                for (idx, tool) in AVAILABLE_TOOLS.iter().copied().enumerate() {
                    {
                        // Skip showing Read/Write file tools - they're enabled automatically when WASI tools are used
                        if tool == ChatToolKind::ReadTextFile || tool == ChatToolKind::WriteTextFile {
                            rsx! {}
                        } else {
                            let enabled = active_tools
                                .read()
                                .iter()
                                .any(|active| active.matches_builtin_kind(tool));
                            let key = format!("tool-{}", idx);
                            rsx! {
                                button {
                                    class: if enabled { "tool-card tool-card--active" } else { "tool-card" },
                                    key: "{key}",
                                    onclick: move |_| on_toggle_tool.call(tool),

                                div { class: "tool-card-icon",
                                    match tool.icon() {
                                        "Search" => rsx! {
                                            svg { xmlns: "http://www.w3.org/2000/svg", view_box: "0 0 24 24",
                                                fill: "none", stroke: "currentColor", stroke_width: "2",
                                                width: "18", height: "18",
                                                circle { cx: "11", cy: "11", r: "7" }
                                                path { d: "M20 20l-3.5-3.5" }
                                            }
                                        },
                                        _ => rsx! { span { "+" } },
                                    }
                                }
                                div { class: "tool-card-copy",
                                    div { class: "tool-card-name", "{tool.label()}" }
                                    div { class: "tool-card-desc", "{tool.description()}" }
                                    div { class: "tool-card-hint",
                                        if enabled {
                                            "Enabled"
                                        } else {
                                            "Tap to enable"
                                        }
                                    }
                                }
                                div { class: if enabled { "tool-card-toggle tool-card-toggle--active" } else { "tool-card-toggle" },
                                    if enabled { "On" } else { "Off" }
                                }
                            }
                            }
                        }
                    }
                }

                // WASI Apps section
                if has_wasi {
                    div { class: "tool-picker-section-header", "WASI Apps" }
                    for (idx, app) in wasi_apps.read().iter().enumerate() {
                        {
                            let app_id = app.id.clone();
                            let app_id3 = app.id.clone();
                            let enabled = active_tools.read().iter().any(|t| t.wasi_app_id.as_ref() == Some(&app_id));
                            let key = format!("wasi-{}", idx);
                            rsx! {
                                button {
                                    class: if enabled { "tool-card tool-card--active" } else { "tool-card" },
                                    key: "{key}",
                                    onclick: move |_| on_toggle_wasi.call(app_id3.clone()),

                                    div { class: "tool-card-icon",
                                        svg { xmlns: "http://www.w3.org/2000/svg", view_box: "0 0 24 24",
                                            fill: "none", stroke: "currentColor", stroke_width: "2",
                                            width: "18", height: "18",
                                            polyline { points: "4 17 10 11 4 5" }
                                            line { x1: "12", y1: "19", x2: "20", y2: "19" }
                                        }
                                    }
                                    div { class: "tool-card-copy",
                                        div { class: "tool-card-name", "{app.name}" }
                                        div { class: "tool-card-desc",
                                            if app.description.is_empty() {
                                                "No description"
                                            } else {
                                                "{app.description}"
                                            }
                                        }
                                    }
                                    div { class: if enabled { "tool-card-toggle tool-card-toggle--active" } else { "tool-card-toggle" },
                                        if enabled { "On" } else { "Off" }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            div { id: "tool-picker-footer",
                div { class: "tool-picker-command-tip",
                    span { "Tools are automatically invoked by the model when needed." }
                }
                button {
                    class: "accent-btn",
                    onclick: move |_| on_close.call(()),
                    "Done"
                }
            }
        }
    }
}
