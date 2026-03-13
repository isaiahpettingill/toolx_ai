use dioxus::prelude::*;

use crate::tools::{ChatToolConfig, ChatToolKind, AVAILABLE_TOOLS};

#[component]
pub fn ToolPickerModal(
    active_tools: Vec<ChatToolConfig>,
    on_toggle_tool: EventHandler<ChatToolKind>,
    on_close: EventHandler<()>,
) -> Element {
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
                    p { id: "tool-picker-subtitle", "Enable lightweight tools that can be invoked from the composer." }
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
                for tool in AVAILABLE_TOOLS.iter().copied() {
                    {
                        let enabled = active_tools.iter().any(|active| active.kind == tool);
                        rsx! {
                            button {
                                class: if enabled { "tool-card tool-card--active" } else { "tool-card" },
                                key: "{tool.id()}",
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
                                            "Enabled for this chat"
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
