use dioxus::document::eval;
use dioxus::prelude::*;

use crate::db::{self, ChatSummary, DbMessage};
use crate::markdown;

const CHAT_CSS: Asset = asset!("/assets/styling/chat.css");

// ── Model definitions ────────────────────────────────────────────────────────

#[derive(Clone, PartialEq, Debug)]
pub struct ModelDef {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
}

pub const MODELS: &[ModelDef] = &[
    ModelDef {
        id: "echo:0b",
        label: "echo:0b",
        description: "Echoes your input back",
    },
    ModelDef {
        id: "reverse:0b",
        label: "reverse:0b",
        description: "Reverses your input",
    },
];

fn run_model(model_id: &str, input: &str) -> String {
    match model_id {
        "reverse:0b" => input.chars().rev().collect(),
        _ => input.to_string(),
    }
}

// ── In-memory message state ──────────────────────────────────────────────────

#[derive(Clone, PartialEq, Debug)]
pub struct UiMessage {
    pub id: String,
    pub role: String,
    pub content: String,
    pub html: String,
}

impl UiMessage {
    fn from_db(msg: &DbMessage) -> Self {
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
        }
    }
}

fn escape_user_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

// ── Root App component ───────────────────────────────────────────────────────

#[component]
pub fn ChatApp() -> Element {
    let accent = use_signal(|| "#3b5bdb".to_string());
    use_context_provider(|| accent);

    let conn = use_signal(|| db::open().expect("Failed to open SQLite database"));

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

    // Drawer state
    let mut drawer_open = use_signal(|| false);

    // Sidebar rename state
    let mut renaming_id: Signal<Option<String>> = use_signal(|| None);
    let mut rename_buf = use_signal(|| String::new());

    // Load messages whenever active chat changes
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
            }
        }
    });

    let new_chat = {
        let conn = conn.clone();
        move |_: MouseEvent| {
            let model = current_model.read().clone();
            match db::create_chat(&conn.read(), "New chat", &model) {
                Ok(chat) => {
                    let id = chat.id.clone();
                    chats.write().insert(0, chat);
                    active_chat_id.set(Some(id));
                    messages.set(Vec::new());
                    drawer_open.set(false);
                }
                Err(e) => eprintln!("Failed to create chat: {e}"),
            }
        }
    };

    rsx! {
        document::Link { rel: "stylesheet", href: CHAT_CSS }
        document::Link {
            rel: "stylesheet",
            href: "https://cdn.jsdelivr.net/npm/katex@0.16.11/dist/katex.min.css"
        }
        document::Script {
            src: "https://cdn.jsdelivr.net/npm/katex@0.16.11/dist/katex.min.js",
            defer: true
        }
        document::Script { {HELPER_JS} }

        div { id: "app-shell",
            style: "--accent: {accent}",

            // ── Mobile header bar ──────────────────────────────────────
            AppHeader {
                drawer_open: drawer_open(),
                on_hamburger: move |_| *drawer_open.write() ^= true,
                on_new_chat: new_chat,
            }

            // ── Body: sidebar + main ───────────────────────────────────
            div { id: "app-body",

                // Tap-outside backdrop (mobile)
                if drawer_open() {
                    div {
                        id: "drawer-backdrop",
                        onclick: move |_| drawer_open.set(false),
                    }
                }

                // Sidebar
                div {
                    id: "sidebar",
                    class: if drawer_open() { "drawer-open" } else { "" },

                    div { id: "sidebar-header",
                        span { id: "sidebar-brand", "chats" }
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
                                let is_renaming = renaming_id.read().as_deref() == Some(&chat.id);
                                let title = chat.title.clone();
                                let row_class = if is_active { "chat-row chat-row--active" } else { "chat-row" };
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
                        ColorPicker { accent }
                    }
                }

                // ── Main content ───────────────────────────────────────
                div { id: "main-area",

                    // Model selector — always visible at the top
                    ModelSelector {
                        conn,
                        current_model,
                        chat_id: active_chat_id().clone(),
                    }

                    if active_chat_id.read().is_some() {
                        ChatPane {
                            conn,
                            chat_id: active_chat_id().clone().unwrap(),
                            messages,
                            current_model,
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
            }
        }
    }
}

// ── Rename helper ────────────────────────────────────────────────────────────

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

// ── Mobile header bar ────────────────────────────────────────────────────────

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

// ── Model selector (always visible above the chat area) ──────────────────────

#[component]
fn ModelSelector(
    conn: Signal<rusqlite::Connection>,
    mut current_model: Signal<String>,
    chat_id: Option<String>,
) -> Element {
    let mut model_open = use_signal(|| false);

    rsx! {
        div { id: "chat-topbar",
            div { id: "model-selector",
                button {
                    id: "model-selector-btn",
                    onclick: move |_| *model_open.write() ^= true,
                    span { "{current_model}" }
                    svg { xmlns: "http://www.w3.org/2000/svg", view_box: "0 0 24 24",
                        fill: "none", stroke: "currentColor", stroke_width: "2",
                        width: "12", height: "12",
                        polyline { points: "6 9 12 15 18 9" }
                    }
                }
                if model_open() {
                    div { id: "model-dropdown",
                        for m in MODELS.iter() {
                            {
                                let mid = m.id;
                                let mlabel = m.label;
                                let mdesc = m.description;
                                let conn2 = conn.clone();
                                let cid = chat_id.clone();
                                let is_active = current_model.read().as_str() == mid;
                                rsx! {
                                    button {
                                        class: if is_active { "model-option model-option--active" } else { "model-option" },
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
    }
}

// ── Color picker ─────────────────────────────────────────────────────────────

const PRESET_COLORS: &[(&str, &str)] = &[
    ("Indigo", "#3b5bdb"),
    ("Violet", "#7048e8"),
    ("Teal", "#0ca678"),
    ("Rose", "#e03131"),
    ("Amber", "#f08c00"),
    ("Sky", "#1098ad"),
];

#[component]
fn ColorPicker(mut accent: Signal<String>) -> Element {
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

// ── Chat pane ────────────────────────────────────────────────────────────────

#[component]
fn ChatPane(
    conn: Signal<rusqlite::Connection>,
    chat_id: String,
    mut messages: Signal<Vec<UiMessage>>,
    current_model: Signal<String>,
    on_messages_changed: EventHandler<()>,
) -> Element {
    let mut input = use_signal(String::new);

    let do_send = {
        let conn = conn.clone();
        let chat_id = chat_id.clone();
        move || {
            let text = input.read().trim().to_string();
            if text.is_empty() {
                return;
            }
            let model = current_model.read().clone();

            if let Ok(user_msg) = db::add_message(&conn.read(), &chat_id, "user", &text) {
                messages.write().push(UiMessage::from_db(&user_msg));
            }

            // Auto-title on first message
            if messages.read().len() == 1 {
                let title: String = text.chars().take(40).collect();
                db::rename_chat(&conn.read(), &chat_id, &title).ok();
            }

            let response_text = run_model(&model, &text);

            if let Ok(asst_msg) =
                db::add_message(&conn.read(), &chat_id, "assistant", &response_text)
            {
                messages.write().push(UiMessage::from_db(&asst_msg));
            }

            input.set(String::new());
            on_messages_changed.call(());
        }
    };

    rsx! {
        div { id: "chat-pane",

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
                        rsx! {
                            div {
                                class: if is_user { "msg-row msg-row--user" } else { "msg-row msg-row--assistant" },
                                key: "{msg_id}",
                                div {
                                    class: if is_user { "msg-bubble msg-bubble--user" } else { "msg-bubble msg-bubble--assistant" },
                                    dangerous_inner_html: "{html_content}",
                                }
                                if !is_user {
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

            div { id: "chat-input-area",
                div { id: "chat-input-bar",
                    textarea {
                        id: "chat-input",
                        placeholder: "Message {current_model}...",
                        rows: "1",
                        value: "{input}",
                        oninput: move |e| input.set(e.value()),
                        onkeydown: {
                            let mut do_send = do_send.clone();
                            move |e: KeyboardEvent| {
                                if e.key() == Key::Enter && e.modifiers().ctrl() {
                                    do_send();
                                }
                            }
                        },
                    }
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
                p { id: "input-hint", "Ctrl+Enter to send" }
            }
        }
    }
}

// ── JS helpers ───────────────────────────────────────────────────────────────

const HELPER_JS: &str = r#"
function copyCode(btn) {
    var code = btn.getAttribute('data-code')
        .replace(/&#10;/g, '\n')
        .replace(/&lt;/g, '<')
        .replace(/&gt;/g, '>')
        .replace(/&amp;/g, '&')
        .replace(/&quot;/g, '"');
    navigator.clipboard.writeText(code).then(function() {
        var orig = btn.textContent;
        btn.textContent = 'Copied!';
        setTimeout(function() { btn.textContent = orig; }, 1500);
    });
}

function autoGrowTextarea(el) {
    el.style.height = 'auto';
    el.style.height = el.scrollHeight + 'px';
}

function renderMath() {
    if (typeof katex === 'undefined') return;
    document.querySelectorAll('.math-inline[data-latex]').forEach(function(el) {
        if (el.dataset.rendered) return;
        try {
            el.innerHTML = katex.renderToString(el.dataset.latex, { throwOnError: false, displayMode: false });
            el.dataset.rendered = '1';
        } catch(e) { el.textContent = el.dataset.latex; }
    });
    document.querySelectorAll('.math-display[data-latex]').forEach(function(el) {
        if (el.dataset.rendered) return;
        try {
            el.innerHTML = katex.renderToString(el.dataset.latex, { throwOnError: false, displayMode: true });
            el.dataset.rendered = '1';
        } catch(e) { el.textContent = el.dataset.latex; }
    });
}

var _mathTimer;
var _observer = new MutationObserver(function() {
    clearTimeout(_mathTimer);
    _mathTimer = setTimeout(function() {
        renderMath();
        var anchor = document.getElementById('scroll-anchor');
        if (anchor) anchor.scrollIntoView({ behavior: 'smooth' });
    }, 50);
});

document.addEventListener('DOMContentLoaded', function() {
    var chat = document.getElementById('chat-messages');
    if (chat) _observer.observe(chat, { childList: true, subtree: true });

    var input = document.getElementById('chat-input');
    if (input) {
        input.addEventListener('input', function() { autoGrowTextarea(this); });
        autoGrowTextarea(input);
    }
});
"#;
