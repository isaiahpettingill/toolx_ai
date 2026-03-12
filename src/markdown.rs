//! Converts markdown (with inline LaTeX and fenced code blocks) to HTML.
//! LaTeX is handled by a simple pass that wraps $...$ and $$...$$ in KaTeX-compatible spans.
//! Syntax highlighting is done via syntect.

use pulldown_cmark::{html, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use syntect::highlighting::ThemeSet;
use syntect::html::highlighted_html_for_string;
use syntect::parsing::SyntaxSet;

/// Render markdown string to an HTML string.
/// Supports: GitHub-flavored markdown, fenced code blocks with syntax highlighting,
/// inline `$...$` LaTeX, and display `$$...$$` LaTeX.
pub fn render(markdown: &str) -> String {
    // Pre-process: protect LaTeX delimiters from the markdown parser by replacing
    // $...$ with placeholder spans, then re-inject after HTML generation.
    // We use a simpler approach: collect code blocks with syntect ourselves,
    // and let pulldown-cmark handle everything else.

    let ss = SyntaxSet::load_defaults_newlines();
    let ts = ThemeSet::load_defaults();
    let theme = &ts.themes["base16-ocean.dark"];

    let opts = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_SMART_PUNCTUATION;

    let parser = Parser::new_ext(markdown, opts);

    // We'll transform events: when we see a code block, replace it with highlighted HTML.
    let mut in_code_block = false;
    let mut code_lang = String::new();
    let mut code_buf = String::new();

    let events: Vec<Event> = parser
        .flat_map(|event| match event {
            Event::Start(Tag::CodeBlock(kind)) => {
                in_code_block = true;
                code_lang = match &kind {
                    CodeBlockKind::Fenced(lang) => lang.split_whitespace().next().unwrap_or("").to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                code_buf.clear();
                vec![]
            }
            Event::Text(text) if in_code_block => {
                code_buf.push_str(&text);
                vec![]
            }
            Event::End(TagEnd::CodeBlock) => {
                in_code_block = false;
                let lang = code_lang.trim();
                let raw_code = code_buf.clone();

                let syntax = if lang.is_empty() {
                    ss.find_syntax_plain_text()
                } else {
                    ss.find_syntax_by_token(lang)
                        .unwrap_or_else(|| ss.find_syntax_plain_text())
                };

                let highlighted = highlighted_html_for_string(&raw_code, &ss, syntax, theme)
                    .unwrap_or_else(|_| format!("<pre><code>{}</code></pre>", escape_html(&raw_code)));

                // Wrap with our custom wrapper that includes copy button and lang label
                let lang_attr = escape_attr(lang);
                let code_escaped = escape_html(&raw_code); // for data attribute
                let wrapper = format!(
                    r#"<div class="code-block" data-lang="{lang_attr}"><div class="code-block-header"><span class="code-lang">{lang_attr}</span><button class="copy-code-btn" data-code="{code_escaped}" onclick="copyCode(this)">Copy</button></div><div class="code-block-body">{highlighted}</div></div>"#,
                );
                vec![Event::Html(wrapper.into())]
            }
            other => vec![other],
        })
        .collect();

    // Generate HTML
    let mut html_output = String::new();
    html::push_html(&mut html_output, events.into_iter());

    // Post-process: convert LaTeX delimiters to KaTeX HTML spans
    let html_output = process_latex(&html_output);

    html_output
}

/// Escape HTML special characters for code content in data attributes
fn escape_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\n', "&#10;")
}

/// Replace $$...$$ and $...$ with KaTeX render spans.
/// We use data attributes and let a small JS snippet in the page invoke KaTeX.
fn process_latex(html: &str) -> String {
    // Display math: $$...$$
    let html = replace_delimited(html, "$$", "$$", |inner| {
        format!(
            r#"<span class="math-display" data-latex="{}">[math]</span>"#,
            escape_attr(inner)
        )
    });
    // Inline math: $...$
    let html = replace_delimited(&html, "$", "$", |inner| {
        // Don't match things that look like currency (no newlines, reasonable length)
        if inner.contains('\n') || inner.len() > 500 {
            format!("${inner}$")
        } else {
            format!(
                r#"<span class="math-inline" data-latex="{}">[math]</span>"#,
                escape_attr(inner)
            )
        }
    });
    html
}

fn replace_delimited(input: &str, open: &str, close: &str, f: impl Fn(&str) -> String) -> String {
    let mut result = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find(open) {
        result.push_str(&rest[..start]);
        let after_open = &rest[start + open.len()..];
        if let Some(end) = after_open.find(close) {
            let inner = &after_open[..end];
            result.push_str(&f(inner));
            rest = &after_open[end + close.len()..];
        } else {
            // No matching close — emit as-is
            result.push_str(open);
            rest = after_open;
        }
    }
    result.push_str(rest);
    result
}
