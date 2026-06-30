use gpui::{AnyElement, App, FontWeight, SharedString, div, prelude::*, px};
use mezon_store::{Message, MessageSpan, PlatformStore};

use super::context::RowCtx;
use crate::theme::Theme;

/// Render a message body from its precomputed rich-text spans (cf. React
/// `MessageContent` -> `MessageLine`).
///
/// Plain-text messages (the vast majority) render as one text element per line
/// — GPUI wraps each line natively — which keeps the per-row element count tiny
/// and scrolling smooth. Only messages with inline tokens (mentions, emoji,
/// links, inline code) fall back to the heavier flex-wrap word layout needed to
/// interleave chips with wrapping text.
pub fn render_message_content(msg: &Message, ctx: &RowCtx) -> AnyElement {
    if msg.spans.is_empty() && !msg.is_edited {
        return div().into_any_element();
    }
    let theme = ctx.theme;

    let has_tokens = msg.spans.iter().any(|s| !matches!(s, MessageSpan::Text(_)));

    // Fast path: plain text, no inline chips.
    if !has_tokens && !msg.is_edited {
        return render_plain_text_spans(&msg.spans, theme.text_primary);
    }

    // Fast path: one or more links with no other inline tokens — GPUI wraps
    // long URLs natively in a full-width block (React `break-words` on links).
    if is_link_only(&msg.spans) && !msg.is_edited {
        return render_link_only_spans(&msg.spans, theme);
    }

    // Rich path: interleave text words with inline chips so the row wraps.
    let mut row = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_baseline()
        .gap_x(px(4.))
        .w_full()
        .min_w_0()
        .text_sm()
        .text_color(theme.text_primary);

    for span in &msg.spans {
        row = append_span(row, span, theme);
    }

    if msg.is_edited {
        row = row.child(
            div()
                .text_color(theme.text_muted)
                .text_size(px(9.))
                .child(mezon_i18n::t(ctx.locale, "message.edited")),
        );
    }

    row.into_any_element()
}

fn append_span(mut row: gpui::Div, span: &MessageSpan, theme: &Theme) -> gpui::Div {
    match span {
        MessageSpan::Text(text) => {
            for child in text_to_words(text, theme) {
                row = row.child(child);
            }
            row
        }
        MessageSpan::Bold(text) => {
            row.child(div().font_weight(FontWeight::BOLD).child(text.clone()))
        }
        MessageSpan::Code(text) => row.child(
            div()
                .px_1()
                .rounded_sm()
                .bg(theme.bg_tertiary)
                .text_color(theme.text_primary)
                .font_family("monospace")
                .child(text.clone()),
        ),
        MessageSpan::CodeBlock { text, .. } => row.child(
            div()
                .w_full()
                .my_1()
                .px_2()
                .py_1()
                .rounded_md()
                .bg(theme.bg_tertiary)
                .text_color(theme.text_primary)
                .font_family("monospace")
                .child(text.clone()),
        ),
        MessageSpan::Link { text, url } => {
            let resolved = resolve_link_url(url, text);
            for child in link_to_wrap_segments(text, resolved, theme.text_link) {
                row = row.child(child);
            }
            row
        }
        MessageSpan::Mention { display, .. } => row.child(
            div()
                .px_0p5()
                .rounded_sm()
                .bg(gpui::Rgba {
                    a: 0.18,
                    ..theme.brand
                })
                .text_color(theme.text_link)
                .child(display.clone()),
        ),
        MessageSpan::Hashtag { display, .. } => row.child(
            div()
                .px_0p5()
                .rounded_sm()
                .bg(gpui::Rgba {
                    a: 0.18,
                    ..theme.brand
                })
                .text_color(theme.text_link)
                .child(display.clone()),
        ),
        MessageSpan::Emoji { name, .. } => row.child(div().child(name.clone())),
    }
}

fn is_link_only(spans: &[MessageSpan]) -> bool {
    spans.iter().any(|s| matches!(s, MessageSpan::Link { .. }))
        && spans.iter().all(|s| match s {
            MessageSpan::Link { .. } => true,
            MessageSpan::Text(text) => text.trim().is_empty(),
            _ => false,
        })
}

fn render_plain_text_spans(spans: &[MessageSpan], color: gpui::Rgba) -> AnyElement {
    let text: String = spans
        .iter()
        .filter_map(|s| match s {
            MessageSpan::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect();
    if text.is_empty() {
        return div().into_any_element();
    }
    if !text.contains('\n') {
        return div()
            .w_full()
            .min_w_0()
            .text_sm()
            .text_color(color)
            .child(text)
            .into_any_element();
    }
    let mut col = div().flex().flex_col().w_full().min_w_0().text_sm();
    for line in text.split('\n') {
        col = col.child(
            div()
                .w_full()
                .min_w_0()
                .min_h(px(20.))
                .text_color(color)
                .child(line.to_string()),
        );
    }
    col.into_any_element()
}

fn render_link_only_spans(spans: &[MessageSpan], theme: &Theme) -> AnyElement {
    let mut col = div().flex().flex_col().w_full().min_w_0().text_sm().gap_1();
    for span in spans {
        let MessageSpan::Link { text, url } = span else {
            continue;
        };
        if text.is_empty() {
            continue;
        }
        let resolved = resolve_link_url(url, text);
        if !text.contains('\n') {
            col = col.child(link_block(resolved, text.clone(), theme.text_link));
            continue;
        }
        for line in text.split('\n') {
            if line.is_empty() {
                continue;
            }
            col = col.child(link_block(resolved.clone(), line.to_string(), theme.text_link));
        }
    }
    col.into_any_element()
}

fn resolve_link_url(url: &str, text: &str) -> String {
    if !url.is_empty() {
        return url.to_string();
    }
    text.to_string()
}

fn open_message_link(url: String, cx: &mut App) {
    if url.is_empty() {
        return;
    }
    if let Some(store) = PlatformStore::try_global(cx) {
        let _ = store.read(cx).open_url_external(&url);
    }
}

fn link_block(url: String, display: String, color: gpui::Rgba) -> AnyElement {
    let id = SharedString::from(format!("msg-link-{url}"));
    div()
        .id(id)
        .w_full()
        .min_w_0()
        .cursor_pointer()
        .text_color(color)
        .on_click(move |_, _, cx| open_message_link(url.clone(), cx))
        .child(display)
        .into_any_element()
}

fn link_segment(url: String, display: String, color: gpui::Rgba) -> AnyElement {
    let id = SharedString::from(format!("msg-link-{url}-{display}"));
    div()
        .id(id)
        .cursor_pointer()
        .text_color(color)
        .on_click(move |_, _, cx| open_message_link(url.clone(), cx))
        .child(display)
        .into_any_element()
}

/// Split a plain-text run into word/break children so it wraps inside a
/// flex-wrap row. Newlines become full-width spacers that force a line break.
fn text_to_words(text: &str, theme: &Theme) -> Vec<AnyElement> {
    let mut out: Vec<AnyElement> = Vec::new();
    let mut first_line = true;
    for line in text.split('\n') {
        if !first_line {
            // Force a wrap to the next line.
            out.push(div().w_full().h_0().into_any_element());
        }
        first_line = false;
        for word in line.split_whitespace() {
            out.push(
                div()
                    .text_color(theme.text_primary)
                    .child(word.to_string())
                    .into_any_element(),
            );
        }
    }
    out
}

/// Split URL / long unbroken strings into flex-wrap segments (React
/// `break-words` / `break-all` on inline links).
fn link_to_wrap_segments(text: &str, url: String, color: gpui::Rgba) -> Vec<AnyElement> {
    let mut out: Vec<AnyElement> = Vec::new();
    let mut first_line = true;
    for line in text.split('\n') {
        if !first_line {
            out.push(div().w_full().h_0().into_any_element());
        }
        first_line = false;
        if line.chars().any(char::is_whitespace) {
            for word in line.split_whitespace() {
                for segment in split_unbreakable(word) {
                    out.push(link_segment(url.clone(), segment, color));
                }
            }
        } else {
            for segment in split_unbreakable(line) {
                out.push(link_segment(url.clone(), segment, color));
            }
        }
    }
    out
}

fn split_unbreakable(text: &str) -> Vec<String> {
    const MAX_SEGMENT_LEN: usize = 32;
    let mut parts = Vec::new();
    let mut buf = String::new();
    for ch in text.chars() {
        buf.push(ch);
        if matches!(
            ch,
            '/' | '-' | '_' | '.' | '?' | '&' | '#' | '=' | '@' | ':'
        ) {
            parts.push(std::mem::take(&mut buf));
        } else if buf.chars().count() >= MAX_SEGMENT_LEN {
            parts.push(std::mem::take(&mut buf));
        }
    }
    if !buf.is_empty() {
        parts.push(buf);
    }
    parts
}
