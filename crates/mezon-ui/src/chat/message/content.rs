use gpui::{AnyElement, FontWeight, div, prelude::*, px};
use mezon_store::{Message, MessageSpan};

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
        let text: String = msg
            .spans
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
                .text_sm()
                .text_color(theme.text_primary)
                .child(text)
                .into_any_element();
        }
        let mut col = div().flex().flex_col().w_full().text_sm();
        for line in text.split('\n') {
            col = col.child(
                div()
                    .w_full()
                    .min_h(px(20.))
                    .text_color(theme.text_primary)
                    .child(line.to_string()),
            );
        }
        return col.into_any_element();
    }

    // Rich path: interleave text words with inline chips so the row wraps.
    let mut row = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_baseline()
        .gap_x(px(4.))
        .w_full()
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
        MessageSpan::Link { text, .. } => {
            row.child(div().text_color(theme.text_link).child(text.clone()))
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
